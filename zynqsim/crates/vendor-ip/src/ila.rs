//! Behavioural model of the Xilinx Integrated Logic Analyzer core (`ila` 6.2, PG172).
//!
//! The Integrated Logic Analyzer (ILA) core is a hardware debugging engine used to
//! capture internal FPGA signals in real-time. In an embedded system or AXI-accessible
//! debug hub, the core is controlled via an AXI4-Lite slave interface to configure
//! trigger comparisons, arm/disarm the acquisition state machine, inspect live probe inputs,
//! and read out captured trace sample buffers.
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Programmable trigger comparison on probe inputs (Equal, Not Equal, Greater, Less) with bitmask;
//! - Immediate trigger capability (`CR_TRIG_NOW`);
//! - Configurable sample buffer depth (default 1024 samples) with sample count and trigger position;
//! - Sequential trace sample buffer readout via address/data port (`TRACE_RADDR`/`TRACE_RDATA`)
//!   and direct memory-mapped buffer window (`0x1000..=0x1000 + 4*depth`);
//! - Autonomous probe stimulus generator (`CR_STIM_EN`) for standalone verification;
//! - Interrupt output on trigger match (`IRQ_TRIGGERED`) and buffer full (`IRQ_DONE`).

use pl_native::RegisterBlock;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Core Register Offsets
const ILA_CR_OFFSET: u64 = 0x000;
const ILA_SR_OFFSET: u64 = 0x004;
const ILA_ISR_OFFSET: u64 = 0x008;
const ILA_IER_OFFSET: u64 = 0x00C;
const ILA_PROBE0_VAL_OFFSET: u64 = 0x010;
const ILA_PROBE0_MASK_OFFSET: u64 = 0x014;
const ILA_PROBE0_MODE_OFFSET: u64 = 0x018;
const ILA_SAMPLE_DEPTH_OFFSET: u64 = 0x020;
const ILA_SAMPLE_CNT_OFFSET: u64 = 0x024;
const ILA_TRIG_POS_OFFSET: u64 = 0x028;
const ILA_VER_OFFSET: u64 = 0x02C;
const ILA_TRACE_RADDR_OFFSET: u64 = 0x030;
const ILA_TRACE_RDATA_OFFSET: u64 = 0x034;
const ILA_PROBE0_LIVE_OFFSET: u64 = 0x038;

// Direct Trace Buffer Window Base
const ILA_TRACE_BUF_BASE: u64 = 0x1000;

// Control Register (CR) Bit Masks
const CR_ARM: u32 = 1 << 0;
const CR_DISARM: u32 = 1 << 1;
const CR_TRIG_NOW: u32 = 1 << 2;
const CR_AUTO_REARM: u32 = 1 << 3;
const CR_RESET: u32 = 1 << 4;
const CR_STIM_EN: u32 = 1 << 5;

// Status Register (SR) Bit Masks
const SR_ARMED: u32 = 1 << 0;
const SR_TRIGGERED: u32 = 1 << 1;
const SR_FULL: u32 = 1 << 2;
const SR_IDLE: u32 = 1 << 3;

// State codes in SR[31:16]
const STATE_CODE_IDLE: u32 = 0x0000;
const STATE_CODE_ARMED: u32 = 0x0001;
const STATE_CODE_TRIGGERED: u32 = 0x0002;
const STATE_CODE_FULL: u32 = 0x0004;

// Interrupt Status & Enable Masks (ISR / IER)
const IRQ_TRIGGERED: u32 = 1 << 0;
const IRQ_DONE: u32 = 1 << 1;

// Core Version (ila 6.2)
const ILA_VERSION_6_2: u32 = 0x0602_0000;

/// Acquisition engine state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IlaState {
    Idle,
    Armed,
    Triggered,
    Full,
}

/// Trigger comparison mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriggerMode {
    Equal = 0,
    NotEqual = 1,
    GreaterThan = 2,
    LessThan = 3,
}

impl TriggerMode {
    fn from_u32(val: u32) -> Self {
        match val & 3 {
            0 => TriggerMode::Equal,
            1 => TriggerMode::NotEqual,
            2 => TriggerMode::GreaterThan,
            _ => TriggerMode::LessThan,
        }
    }
}

/// Static configuration for an Integrated Logic Analyzer core instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IlaConfig {
    /// Number of probe inputs (1..64, default 1).
    pub num_probes: u32,
    /// Bit width of probe 0 (1..4096, default 32).
    pub probe0_width: u32,
    /// Sample buffer storage depth (default 1024).
    pub data_depth: usize,
    /// Storage qualification enabled.
    pub storage_qual: bool,
    /// Nominal sampling clock frequency in Hertz.
    pub clk_freq_hz: u64,
    /// Address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for IlaConfig {
    fn default() -> Self {
        Self {
            num_probes: 1,
            probe0_width: 32,
            data_depth: 1024,
            storage_qual: false,
            clk_freq_hz: 100_000_000,
            window: 0x10000,
        }
    }
}

impl IlaConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = IlaConfig {
            window: window.max(0x1000),
            ..IlaConfig::default()
        };

        if let Some(v) = params.get("C_NUM_DATA_PROBE") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.num_probes = n;
                }
            }
        }
        if let Some(v) = params.get("C_PROBE0_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.probe0_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_DATA_DEPTH") {
            if let Ok(n) = v.trim().parse::<usize>() {
                if (16..=131072).contains(&n) {
                    cfg.data_depth = n;
                }
            }
        }
        if let Some(v) = params.get("C_EN_STRG_QUAL") {
            cfg.storage_qual = matches!(v.trim(), "1" | "true" | "TRUE");
        }
        if let Some(v) = params.get("CLK_FREQ_HZ") {
            if let Ok(n) = v.trim().parse::<u64>() {
                if n > 0 {
                    cfg.clk_freq_hz = n;
                }
            }
        }

        Ok(cfg)
    }
}

/// Dynamic statistics and performance counters for an ILA core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IlaStats {
    /// Number of acquisitions successfully completed.
    pub captures_completed: u64,
    /// Number of trigger match events detected.
    pub triggers_fired: u64,
    /// Number of sample words recorded.
    pub samples_recorded: u64,
}

/// Internal mutable state of the core.
struct State {
    cfg: IlaConfig,
    now: u64,
    clock_acc_ps: u64,

    // Registers
    cr: u32,
    isr: u32,
    ier: u32,
    probe0_val: u32,
    probe0_mask: u32,
    probe0_mode: TriggerMode,
    trace_raddr: usize,

    // Acquisition state machine
    state: IlaState,
    live_probe0: u32,
    trig_pos: usize,
    sample_buffer: Vec<u32>,
    stimulus_counter: u32,
    external_probe_queue: VecDeque<u32>,

    // Statistics
    stats: IlaStats,
}

impl State {
    fn new(cfg: IlaConfig) -> Self {
        Self {
            cfg,
            now: 0,
            clock_acc_ps: 0,
            cr: 0,
            isr: 0,
            ier: 0,
            probe0_val: 0,
            probe0_mask: 0xFFFF_FFFF,
            probe0_mode: TriggerMode::Equal,
            trace_raddr: 0,
            state: IlaState::Idle,
            live_probe0: 0,
            trig_pos: 0,
            sample_buffer: Vec::new(),
            stimulus_counter: 0,
            external_probe_queue: VecDeque::new(),
            stats: IlaStats::default(),
        }
    }

    fn reset(&mut self) {
        self.cr = 0;
        self.isr = 0;
        self.ier = 0;
        self.probe0_val = 0;
        self.probe0_mask = 0xFFFF_FFFF;
        self.probe0_mode = TriggerMode::Equal;
        self.trace_raddr = 0;
        self.state = IlaState::Idle;
        self.live_probe0 = 0;
        self.trig_pos = 0;
        self.sample_buffer.clear();
        self.stimulus_counter = 0;
        self.external_probe_queue.clear();
        self.clock_acc_ps = 0;
    }

    fn is_armed(&self) -> bool {
        self.state == IlaState::Armed || self.state == IlaState::Triggered
    }

    fn is_triggered(&self) -> bool {
        self.state == IlaState::Triggered || self.state == IlaState::Full
    }

    fn is_full(&self) -> bool {
        self.state == IlaState::Full
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
    }

    fn status_code(&self) -> u32 {
        match self.state {
            IlaState::Idle => STATE_CODE_IDLE,
            IlaState::Armed => STATE_CODE_ARMED,
            IlaState::Triggered => STATE_CODE_TRIGGERED,
            IlaState::Full => STATE_CODE_FULL,
        }
    }

    fn read_status(&self) -> u32 {
        let mut sr = 0u32;
        if self.is_armed() {
            sr |= SR_ARMED;
        }
        if self.is_triggered() {
            sr |= SR_TRIGGERED;
        }
        if self.is_full() {
            sr |= SR_FULL;
        }
        if self.state == IlaState::Idle {
            sr |= SR_IDLE;
        }
        sr |= self.status_code() << 16;
        sr
    }

    fn check_trigger_match(&self, sample: u32) -> bool {
        let masked_sample = sample & self.probe0_mask;
        let masked_val = self.probe0_val & self.probe0_mask;
        match self.probe0_mode {
            TriggerMode::Equal => masked_sample == masked_val,
            TriggerMode::NotEqual => masked_sample != masked_val,
            TriggerMode::GreaterThan => masked_sample > masked_val,
            TriggerMode::LessThan => masked_sample < masked_val,
        }
    }

    fn advance_time(&mut self, now: u64) {
        if now <= self.now {
            return;
        }
        let delta_ps = now - self.now;
        self.now = now;

        let clk_hz = self.cfg.clk_freq_hz.max(1_000_000);
        let ps_per_cycle = (1_000_000_000_000u64 + clk_hz / 2) / clk_hz;

        self.clock_acc_ps += delta_ps;
        let cycles_to_advance = self.clock_acc_ps / ps_per_cycle;
        if cycles_to_advance == 0 {
            return;
        }
        self.clock_acc_ps %= ps_per_cycle;

        for _ in 0..cycles_to_advance {
            // Determine probe input sample: external queue, autonomous stimulus counter, or live value
            let sample = if let Some(ext) = self.external_probe_queue.pop_front() {
                self.live_probe0 = ext;
                ext
            } else if (self.cr & CR_STIM_EN) != 0 {
                self.stimulus_counter = self.stimulus_counter.wrapping_add(1);
                let s = (self.stimulus_counter & 0xFFFF)
                    | ((self.stimulus_counter.wrapping_mul(3)) << 16);
                self.live_probe0 = s;
                s
            } else {
                self.live_probe0
            };

            match self.state {
                IlaState::Idle => {
                    // Not acquiring
                }
                IlaState::Armed => {
                    // Record sample into circular/pre-trigger buffer
                    self.sample_buffer.push(sample);
                    self.stats.samples_recorded += 1;

                    if self.check_trigger_match(sample) {
                        self.state = IlaState::Triggered;
                        self.trig_pos = self.sample_buffer.len().saturating_sub(1);
                        self.isr |= IRQ_TRIGGERED;
                        self.stats.triggers_fired += 1;
                    } else if self.sample_buffer.len() > self.cfg.data_depth / 2 {
                        // Keep buffer bounded before trigger
                        self.sample_buffer.remove(0);
                    }
                }
                IlaState::Triggered => {
                    // Post-trigger acquisition
                    self.sample_buffer.push(sample);
                    self.stats.samples_recorded += 1;

                    if self.sample_buffer.len() >= self.cfg.data_depth {
                        self.state = IlaState::Full;
                        self.isr |= IRQ_DONE;
                        self.stats.captures_completed += 1;

                        if (self.cr & CR_AUTO_REARM) != 0 {
                            self.sample_buffer.clear();
                            self.state = IlaState::Armed;
                        }
                    }
                }
                IlaState::Full => {
                    // Buffer full; waiting for read or reset/rearm
                }
            }
        }
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        if (ILA_TRACE_BUF_BASE..ILA_TRACE_BUF_BASE + 4 * (self.cfg.data_depth as u64))
            .contains(&offset)
        {
            let idx = ((offset - ILA_TRACE_BUF_BASE) / 4) as usize;
            return self.sample_buffer.get(idx).copied().unwrap_or(0);
        }

        match offset {
            ILA_CR_OFFSET => self.cr,
            ILA_SR_OFFSET => self.read_status(),
            ILA_ISR_OFFSET => self.isr,
            ILA_IER_OFFSET => self.ier,
            ILA_PROBE0_VAL_OFFSET => self.probe0_val,
            ILA_PROBE0_MASK_OFFSET => self.probe0_mask,
            ILA_PROBE0_MODE_OFFSET => self.probe0_mode as u32,
            ILA_SAMPLE_DEPTH_OFFSET => self.cfg.data_depth as u32,
            ILA_SAMPLE_CNT_OFFSET => self.sample_buffer.len() as u32,
            ILA_TRIG_POS_OFFSET => self.trig_pos as u32,
            ILA_VER_OFFSET => ILA_VERSION_6_2,
            ILA_TRACE_RADDR_OFFSET => self.trace_raddr as u32,
            ILA_TRACE_RDATA_OFFSET => {
                let val = self
                    .sample_buffer
                    .get(self.trace_raddr)
                    .copied()
                    .unwrap_or(0);
                self.trace_raddr = (self.trace_raddr + 1) % self.cfg.data_depth.max(1);
                val
            }
            ILA_PROBE0_LIVE_OFFSET => self.live_probe0,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            ILA_CR_OFFSET => {
                if (value & CR_RESET) != 0 {
                    self.reset();
                    return;
                }
                self.cr = value & (CR_AUTO_REARM | CR_STIM_EN);

                if (value & CR_DISARM) != 0 {
                    self.state = IlaState::Idle;
                } else if (value & CR_ARM) != 0 {
                    self.sample_buffer.clear();
                    self.trace_raddr = 0;
                    self.state = IlaState::Armed;
                }

                if (value & CR_TRIG_NOW) != 0 && self.state == IlaState::Armed {
                    self.state = IlaState::Triggered;
                    self.trig_pos = self.sample_buffer.len().saturating_sub(1);
                    self.isr |= IRQ_TRIGGERED;
                    self.stats.triggers_fired += 1;
                }
            }
            ILA_ISR_OFFSET => {
                // Write-1-to-clear
                self.isr &= !value;
            }
            ILA_IER_OFFSET => {
                self.ier = value & (IRQ_TRIGGERED | IRQ_DONE);
            }
            ILA_PROBE0_VAL_OFFSET => {
                self.probe0_val = value;
            }
            ILA_PROBE0_MASK_OFFSET => {
                self.probe0_mask = value;
            }
            ILA_PROBE0_MODE_OFFSET => {
                self.probe0_mode = TriggerMode::from_u32(value);
            }
            ILA_TRACE_RADDR_OFFSET => {
                self.trace_raddr = (value as usize) % self.cfg.data_depth.max(1);
            }
            _ => {}
        }
    }
}

/// Shared handle for host inspection, test manipulation, and metrics.
#[derive(Clone)]
pub struct IlaHandle(Arc<Mutex<State>>);

impl IlaHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the live level of probe 0.
    pub fn set_probe0(&self, val: u32) {
        self.lock().live_probe0 = val;
    }

    /// Pushes an external probe 0 sample value into the queue.
    pub fn push_probe_sample(&self, val: u32) {
        self.lock().external_probe_queue.push_back(val);
    }

    /// Forces an immediate trigger.
    pub fn force_trigger(&self) {
        let mut s = self.lock();
        if s.state == IlaState::Armed {
            s.state = IlaState::Triggered;
            s.trig_pos = s.sample_buffer.len().saturating_sub(1);
            s.isr |= IRQ_TRIGGERED;
            s.stats.triggers_fired += 1;
        }
    }

    /// Returns whether the ILA acquisition engine is currently armed.
    pub fn is_armed(&self) -> bool {
        self.lock().is_armed()
    }

    /// Returns whether the ILA acquisition buffer is full.
    pub fn is_full(&self) -> bool {
        self.lock().is_full()
    }

    /// Returns whether the ILA trigger has matched.
    pub fn is_triggered(&self) -> bool {
        self.lock().is_triggered()
    }

    /// Returns a copy of the captured sample buffer.
    pub fn captured_samples(&self) -> Vec<u32> {
        self.lock().sample_buffer.clone()
    }

    /// Returns cumulative statistics.
    pub fn stats(&self) -> IlaStats {
        self.lock().stats
    }
}

/// Integrated Logic Analyzer core model implementing [`RegisterBlock`].
pub struct AxiIla {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiIla {
    /// Constructs a new Integrated Logic Analyzer instance.
    pub fn new(name: &str, cfg: IlaConfig) -> (Self, IlaHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = IlaHandle(Arc::clone(&state));
        let block = Self {
            name: name.to_string(),
            state,
        };
        (block, handle)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RegisterBlock for AxiIla {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        self.lock().read_reg(offset)
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        self.lock().write_reg(offset, value, strobe);
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        let mut s = self.lock();
        s.advance_time(now_ns.saturating_mul(1000));
    }

    fn irq_level(&self) -> bool {
        self.irq_output(0)
    }

    fn irq_output(&self, output: usize) -> bool {
        if output == 0 {
            self.lock().irq()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state_and_version() {
        let (mut dev, handle) = AxiIla::new("ila_0", IlaConfig::default());
        assert_eq!(dev.size(), 0x10000);
        assert_eq!(dev.read32(ILA_VER_OFFSET), ILA_VERSION_6_2);
        assert_eq!(dev.read32(ILA_CR_OFFSET), 0);
        let sr = dev.read32(ILA_SR_OFFSET);
        assert_ne!(sr & SR_IDLE, 0);
        assert_eq!(sr & SR_ARMED, 0);
        assert!(!handle.is_armed());
        assert!(!handle.is_full());
    }

    #[test]
    fn software_reset_clears_registers() {
        let (mut dev, _) = AxiIla::new("ila_0", IlaConfig::default());
        dev.write32(ILA_PROBE0_VAL_OFFSET, 0x1234_5678, 0xF);
        dev.write32(ILA_IER_OFFSET, IRQ_TRIGGERED | IRQ_DONE, 0xF);
        dev.write32(ILA_CR_OFFSET, CR_ARM | CR_STIM_EN, 0xF);
        assert_ne!(dev.read32(ILA_IER_OFFSET), 0);

        // Assert software reset
        dev.write32(ILA_CR_OFFSET, CR_RESET, 0xF);
        assert_eq!(dev.read32(ILA_CR_OFFSET), 0);
        assert_eq!(dev.read32(ILA_IER_OFFSET), 0);
        assert_eq!(dev.read32(ILA_PROBE0_VAL_OFFSET), 0);
    }

    #[test]
    fn immediate_trigger_captures_buffer() {
        let cfg = IlaConfig {
            data_depth: 64,
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, handle) = AxiIla::new("ila_0", cfg);

        dev.write32(ILA_IER_OFFSET, IRQ_TRIGGERED | IRQ_DONE, 0xF);
        // Arm and trigger immediately with stimulus
        dev.write32(ILA_CR_OFFSET, CR_ARM | CR_TRIG_NOW | CR_STIM_EN, 0xF);

        // Advance 100 clock cycles (1000 ns)
        dev.advance(1000);

        assert!(handle.is_full());
        let sr = dev.read32(ILA_SR_OFFSET);
        assert_ne!(sr & SR_FULL, 0);
        assert_ne!(sr & SR_TRIGGERED, 0);
        assert!(dev.irq_level());

        let samples = handle.captured_samples();
        assert_eq!(samples.len(), 64);
    }

    #[test]
    fn probe_compare_trigger_and_trace_readout() {
        let cfg = IlaConfig {
            data_depth: 32,
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, handle) = AxiIla::new("ila_0", cfg);

        // Set trigger compare value: 0x0000_0005 with mask 0x0000_000F
        dev.write32(ILA_PROBE0_VAL_OFFSET, 0x05, 0xF);
        dev.write32(ILA_PROBE0_MASK_OFFSET, 0x0F, 0xF);
        dev.write32(ILA_PROBE0_MODE_OFFSET, 0, 0xF); // Equal
        dev.write32(ILA_IER_OFFSET, IRQ_DONE, 0xF);

        // Arm
        dev.write32(ILA_CR_OFFSET, CR_ARM, 0xF);
        assert!(!handle.is_triggered());

        // Feed samples through handle
        for i in 1..=4 {
            handle.push_probe_sample(i);
        }
        dev.advance(40); // 4 cycles
        assert!(!handle.is_triggered());

        // Feed sample that matches trigger: 0x15 (matches & 0xF == 5)
        handle.push_probe_sample(0x15);
        dev.advance(50); // absolute time: 50 ns
        assert!(handle.is_triggered());

        // Feed remaining samples to complete buffer
        for i in 6..=40 {
            handle.push_probe_sample(i);
        }
        dev.advance(450); // absolute time: 450 ns

        assert!(handle.is_full());
        assert!(dev.irq_level());

        // Verify sequential readout via TRACE_RADDR / TRACE_RDATA
        dev.write32(ILA_TRACE_RADDR_OFFSET, 0, 0xF);
        let first_sample = dev.read32(ILA_TRACE_RDATA_OFFSET);
        assert_eq!(first_sample, 1);

        // Direct window read
        let direct = dev.read32(ILA_TRACE_BUF_BASE);
        assert_eq!(direct, 1);
    }

    #[test]
    fn disarm_aborts_acquisition() {
        let (mut dev, handle) = AxiIla::new("ila_0", IlaConfig::default());
        dev.write32(ILA_CR_OFFSET, CR_ARM, 0xF);
        assert!(handle.is_armed());

        dev.write32(ILA_CR_OFFSET, CR_DISARM, 0xF);
        assert!(!handle.is_armed());
        let sr = dev.read32(ILA_SR_OFFSET);
        assert_ne!(sr & SR_IDLE, 0);
    }
}
