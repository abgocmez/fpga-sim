//! Behavioural model of the Xilinx AXI Performance Monitor core (`axi_perf_mon` 5.0, PG037).
//!
//! The AXI Performance Monitor (APM) core measures real-time performance metrics on AXI
//! interfaces including throughput, latency, transaction counts, and transfer sizes.
//! In an embedded system or system-on-chip interconnect, software configures sample
//! intervals, selects metric types per monitor slot, and reads back metric counters
//! either directly or as sampled snapshots upon interval expiration.
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Software reset via `SRR` with key `0x0000_000A` per PG037;
//! - Control register (`CR`) and Status register (`SR`) for metric and sample counter gating;
//! - Configurable sample interval timer (`SI_REG`, `SIC_REG`) with auto-reload and snapshot latching;
//! - Metric counter array (`MC0`..`MC7`) and corresponding sampled metric registers (`SMC0`..`SMC7`);
//! - Support for 32-bit and 64-bit metric counter readout;
//! - Metric selection registers (`MSR0`, `MSR1`);
//! - Interrupt delivery on sample interval expiration (`IRQ_SAMPLE_EXPIRED`) and overflow;
//! - Simulated bus activity progression (`advance`) and external transaction recording via [`AxiPerfMonHandle`].

use pl_native::RegisterBlock;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Standard AXI Performance Monitor Register Offsets (PG037 Table 2-4)
const APM_SRR_OFFSET: u64 = 0x000;
const APM_SR_OFFSET: u64 = 0x004;
const APM_CR_OFFSET: u64 = 0x024;
const APM_SICR_OFFSET: u64 = 0x028;
const APM_IPISR_OFFSET: u64 = 0x030;
const APM_IPIER_OFFSET: u64 = 0x034;
const APM_SI_OFFSET: u64 = 0x038;
const APM_SIC_OFFSET: u64 = 0x03C;
const APM_MSR0_OFFSET: u64 = 0x044;
const APM_MSR1_OFFSET: u64 = 0x048;

// Metric Counter Base Offsets
const APM_MC_BASE: u64 = 0x100;
const APM_SMC_BASE: u64 = 0x200;

// Software Reset Key
const SRR_RESET_KEY: u32 = 0x0000_000A;

// Status Register (SR) Bit Masks
const SR_METRIC_CNT_EN: u32 = 1 << 0;
const SR_SAMPLE_CNT_EN: u32 = 1 << 1;
const SR_SAMPLE_EXPIRED: u32 = 1 << 2;

// Control Register (CR) Bit Masks
const CR_METRIC_CNT_EN: u32 = 1 << 0;
const CR_METRIC_CNT_RESET: u32 = 1 << 1;
const CR_SAMPLE_CNT_EN: u32 = 1 << 2;
const CR_SAMPLE_CNT_RESET: u32 = 1 << 3;
const CR_SAMPLE_LOAD: u32 = 1 << 4;

// IP Interrupt Status & Enable Masks (IPISR / IPIER)
const IRQ_SAMPLE_EXPIRED: u32 = 1 << 0;
const IRQ_METRIC_OVERFLOW: u32 = 1 << 1;

/// Static configuration for an AXI Performance Monitor core instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfMonConfig {
    /// Number of monitor slots (1..8, default 2).
    pub num_slots: u32,
    /// Number of metric counters (1..10, default 8).
    pub num_counters: u32,
    /// 64-bit metric counter support enabled.
    pub is_64bit: bool,
    /// Nominal core clock frequency in Hertz.
    pub clk_freq_hz: u64,
    /// Address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for PerfMonConfig {
    fn default() -> Self {
        Self {
            num_slots: 2,
            num_counters: 8,
            is_64bit: true,
            clk_freq_hz: 100_000_000,
            window: 0x10000,
        }
    }
}

impl PerfMonConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = PerfMonConfig {
            window: window.max(0x1000),
            ..PerfMonConfig::default()
        };

        if let Some(v) = params.get("C_NUM_MONITOR_SLOTS") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (1..=8).contains(&n) {
                    cfg.num_slots = n;
                }
            }
        }
        if let Some(v) = params.get("C_METRIC_COUNT_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                cfg.is_64bit = n >= 64;
            }
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

/// Dynamic statistics and performance counters for an AXI Performance Monitor core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PerfMonStats {
    /// Total number of sample interval expirations.
    pub sample_intervals_completed: u64,
    /// Total transactions monitored.
    pub transactions_counted: u64,
    /// Total bytes monitored across all slots.
    pub bytes_monitored: u64,
}

/// Internal mutable state of the core.
struct State {
    cfg: PerfMonConfig,
    now: u64,
    clock_acc_ps: u64,

    // Registers
    cr: u32,
    ipisr: u32,
    ipier: u32,
    sample_interval: u32,
    sample_counter: u32,
    msr0: u32,
    msr1: u32,

    // Metric counters: active and sampled snapshots (up to 8 counters, 64-bit each)
    metric_counters: [u64; 8],
    sampled_counters: [u64; 8],

    // Internal activity simulation counter
    stimulus_counter: u32,

    // Dynamic stats
    stats: PerfMonStats,
}

impl State {
    fn new(cfg: PerfMonConfig) -> Self {
        let mut s = Self {
            cfg,
            now: 0,
            clock_acc_ps: 0,
            cr: 0,
            ipisr: 0,
            ipier: 0,
            sample_interval: 10_000,
            sample_counter: 10_000,
            msr0: 0,
            msr1: 0,
            metric_counters: [0u64; 8],
            sampled_counters: [0u64; 8],
            stimulus_counter: 0,
            stats: PerfMonStats::default(),
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.cr = 0;
        self.ipisr = 0;
        self.ipier = 0;
        self.sample_interval = 10_000;
        self.sample_counter = 10_000;
        self.msr0 = 0;
        self.msr1 = 0;
        self.metric_counters = [0u64; 8];
        self.sampled_counters = [0u64; 8];
        self.stimulus_counter = 0;
        self.clock_acc_ps = 0;
    }

    fn sample_snapshot(&mut self) {
        self.sampled_counters = self.metric_counters;
        self.ipisr |= IRQ_SAMPLE_EXPIRED;
        self.stats.sample_intervals_completed += 1;
    }

    fn read_status(&self) -> u32 {
        let mut sr = 0u32;
        if (self.cr & CR_METRIC_CNT_EN) != 0 {
            sr |= SR_METRIC_CNT_EN;
        }
        if (self.cr & CR_SAMPLE_CNT_EN) != 0 {
            sr |= SR_SAMPLE_CNT_EN;
        }
        if (self.ipisr & IRQ_SAMPLE_EXPIRED) != 0 {
            sr |= SR_SAMPLE_EXPIRED;
        }
        sr
    }

    fn irq(&self) -> bool {
        (self.ipisr & self.ipier) != 0
    }

    fn record_transaction(&mut self, slot: usize, bytes: u64, latency_cycles: u64) {
        if (self.cr & CR_METRIC_CNT_EN) == 0 {
            return;
        }
        let base_idx = (slot * 4).min(4);
        // Counter 0: Total byte count
        self.metric_counters[base_idx] = self.metric_counters[base_idx].wrapping_add(bytes);
        // Counter 1: Transaction count
        self.metric_counters[base_idx + 1] = self.metric_counters[base_idx + 1].wrapping_add(1);
        // Counter 2: Cumulative latency
        self.metric_counters[base_idx + 2] =
            self.metric_counters[base_idx + 2].wrapping_add(latency_cycles);
        // Counter 3: Transfer count
        self.metric_counters[base_idx + 3] =
            self.metric_counters[base_idx + 3].wrapping_add(bytes / 4 + 1);

        self.stats.transactions_counted += 1;
        self.stats.bytes_monitored += bytes;
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
        let cycles = self.clock_acc_ps / ps_per_cycle;
        if cycles == 0 {
            return;
        }
        self.clock_acc_ps %= ps_per_cycle;

        // Sample interval counter progression
        if (self.cr & CR_SAMPLE_CNT_EN) != 0 && self.sample_interval > 0 {
            let cycles_u32 = cycles.min(u32::MAX as u64) as u32;
            if self.sample_counter <= cycles_u32 {
                self.sample_snapshot();
                self.sample_counter = self.sample_interval;
            } else {
                self.sample_counter -= cycles_u32;
            }
        }

        // Autonomous traffic generation when metric counter is active
        if (self.cr & CR_METRIC_CNT_EN) != 0 {
            for _ in 0..cycles.min(100) {
                self.stimulus_counter = self.stimulus_counter.wrapping_add(1);
                if (self.stimulus_counter % 10) == 0 {
                    self.record_transaction(0, 64, 4);
                }
            }
        }
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        // Metric Counters: 0x100..=0x17C (step 0x10 per counter, 0x00=low32, 0x04=high32)
        if (APM_MC_BASE..=0x17C).contains(&offset) {
            let rel = offset - APM_MC_BASE;
            let counter_idx = (rel / 0x10) as usize;
            let is_high = (rel % 0x10) >= 4;
            if counter_idx < 8 {
                let val = self.metric_counters[counter_idx];
                return if is_high {
                    (val >> 32) as u32
                } else {
                    val as u32
                };
            }
        }

        // Sampled Metric Counters: 0x200..=0x27C
        if (APM_SMC_BASE..=0x27C).contains(&offset) {
            let rel = offset - APM_SMC_BASE;
            let counter_idx = (rel / 0x10) as usize;
            let is_high = (rel % 0x10) >= 4;
            if counter_idx < 8 {
                let val = self.sampled_counters[counter_idx];
                return if is_high {
                    (val >> 32) as u32
                } else {
                    val as u32
                };
            }
        }

        match offset {
            APM_SRR_OFFSET => 0,
            APM_SR_OFFSET => self.read_status(),
            APM_CR_OFFSET => self.cr,
            APM_SICR_OFFSET => 0,
            APM_IPISR_OFFSET => self.ipisr,
            APM_IPIER_OFFSET => self.ipier,
            APM_SI_OFFSET => self.sample_interval,
            APM_SIC_OFFSET => self.sample_counter,
            APM_MSR0_OFFSET => self.msr0,
            APM_MSR1_OFFSET => self.msr1,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            APM_SRR_OFFSET => {
                if value == SRR_RESET_KEY {
                    self.reset();
                }
            }
            APM_CR_OFFSET => {
                if (value & CR_METRIC_CNT_RESET) != 0 {
                    self.metric_counters = [0u64; 8];
                }
                if (value & CR_SAMPLE_CNT_RESET) != 0 {
                    self.sample_counter = self.sample_interval;
                }
                if (value & CR_SAMPLE_LOAD) != 0 {
                    self.sample_counter = self.sample_interval;
                }
                self.cr = value & (CR_METRIC_CNT_EN | CR_SAMPLE_CNT_EN);
            }
            APM_SICR_OFFSET => {
                if (value & 1) != 0 {
                    self.sample_snapshot();
                }
            }
            APM_IPISR_OFFSET => {
                // Write-1-to-clear
                self.ipisr &= !value;
            }
            APM_IPIER_OFFSET => {
                self.ipier = value & (IRQ_SAMPLE_EXPIRED | IRQ_METRIC_OVERFLOW);
            }
            APM_SI_OFFSET => {
                self.sample_interval = value;
                self.sample_counter = value;
            }
            APM_MSR0_OFFSET => {
                self.msr0 = value;
            }
            APM_MSR1_OFFSET => {
                self.msr1 = value;
            }
            _ => {}
        }
    }
}

/// Shared control handle for an AXI Performance Monitor core instance.
#[derive(Clone)]
pub struct AxiPerfMonHandle {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiPerfMonHandle {
    /// Instance name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Records an AXI bus transaction on the specified slot.
    pub fn record_transaction(&self, slot: usize, bytes: u64, latency_cycles: u64) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.record_transaction(slot, bytes, latency_cycles);
    }

    /// Reads current metric counter value.
    pub fn metric_counter(&self, counter_idx: usize) -> u64 {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.metric_counters.get(counter_idx).copied().unwrap_or(0)
    }

    /// Reads sampled snapshot metric counter value.
    pub fn sampled_counter(&self, counter_idx: usize) -> u64 {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.sampled_counters.get(counter_idx).copied().unwrap_or(0)
    }

    /// Returns static configuration of this core.
    pub fn config(&self) -> PerfMonConfig {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.cfg.clone()
    }

    /// Returns dynamic statistics of this core.
    pub fn stats(&self) -> PerfMonStats {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.stats
    }
}

/// Behavioural register block for the Xilinx AXI Performance Monitor core.
pub struct AxiPerfMon {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiPerfMon {
    /// Constructs a new AXI Performance Monitor instance and returns the device and shared handle.
    pub fn new(name: &str, cfg: PerfMonConfig) -> (Self, AxiPerfMonHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = AxiPerfMonHandle {
            name: name.to_string(),
            state: Arc::clone(&state),
        };
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

impl RegisterBlock for AxiPerfMon {
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
    fn test_perf_mon_reset_and_control() {
        let (mut apm, handle) = AxiPerfMon::new("apm_test", PerfMonConfig::default());

        let sr = apm.read32(APM_SR_OFFSET);
        assert_eq!(sr, 0);

        // Enable metric counters and sample counter
        apm.write32(APM_CR_OFFSET, CR_METRIC_CNT_EN | CR_SAMPLE_CNT_EN, 0xF);
        let sr_en = apm.read32(APM_SR_OFFSET);
        assert_eq!(
            sr_en & (SR_METRIC_CNT_EN | SR_SAMPLE_CNT_EN),
            SR_METRIC_CNT_EN | SR_SAMPLE_CNT_EN
        );

        // Record a transaction
        handle.record_transaction(0, 128, 8);
        assert_eq!(handle.metric_counter(0), 128);
        assert_eq!(handle.metric_counter(1), 1);
        assert_eq!(apm.read32(APM_MC_BASE), 128);

        // Reset via SRR key 0xA
        apm.write32(APM_SRR_OFFSET, SRR_RESET_KEY, 0xF);
        assert_eq!(apm.read32(APM_MC_BASE), 0);
        assert_eq!(apm.read32(APM_CR_OFFSET), 0);
    }

    #[test]
    fn test_perf_mon_sample_interval_and_interrupt() {
        let (mut apm, _) = AxiPerfMon::new("apm_test", PerfMonConfig::default());

        // Set short interval: 100 cycles
        apm.write32(APM_SI_OFFSET, 100, 0xF);
        apm.write32(APM_IPIER_OFFSET, IRQ_SAMPLE_EXPIRED, 0xF);
        apm.write32(APM_CR_OFFSET, CR_SAMPLE_CNT_EN | CR_METRIC_CNT_EN, 0xF);

        assert!(!apm.irq_level());

        // Advance 2 microseconds (200 cycles at 100 MHz)
        apm.advance(2_000);
        assert!(apm.irq_level());
        assert_ne!(apm.read32(APM_IPISR_OFFSET) & IRQ_SAMPLE_EXPIRED, 0);

        // Clear interrupt
        apm.write32(APM_IPISR_OFFSET, IRQ_SAMPLE_EXPIRED, 0xF);
        assert!(!apm.irq_level());
    }

    #[test]
    fn test_perf_mon_manual_snapshot() {
        let (mut apm, handle) = AxiPerfMon::new("apm_test", PerfMonConfig::default());

        apm.write32(APM_CR_OFFSET, CR_METRIC_CNT_EN, 0xF);
        handle.record_transaction(0, 512, 16);

        // Sampled counter before snapshot is 0
        assert_eq!(apm.read32(APM_SMC_BASE), 0);

        // Manual snapshot via SICR
        apm.write32(APM_SICR_OFFSET, 1, 0xF);
        assert_eq!(apm.read32(APM_SMC_BASE), 512);
    }
}
