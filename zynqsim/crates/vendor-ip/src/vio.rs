//! Behavioural model of the Xilinx Virtual Input/Output core (`vio` 3.0, PG159).
//!
//! The Virtual Input/Output (VIO) core is a hardware debugging engine used to monitor
//! and drive internal FPGA signals in real time. In an embedded system or AXI-accessible
//! debug hub, the core is controlled via an AXI4-Lite slave interface to read live or latched
//! probe input signals, update virtual probe output signals, detect activity or toggling on
//! inputs, and trigger interrupts on state transitions.
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Configurable number of input and output probes (up to 8 probes, default 2 each);
//! - Configurable probe bit widths and initial output reset values;
//! - Direct and staged output probe updates (`CR_UPDATE_OUT`, `CR_AUTO_UPDATE`);
//! - Input probe latching (`CR_LATCH_IN`) and live input reading;
//! - Activity and toggle detection flags (`PROBE_IN_ACT`) with write-1-to-clear semantics;
//! - Autonomous loopback mode (`CR_LOOPBACK_EN`) and stimulus generator (`CR_STIM_EN`);
//! - Interrupt delivery on probe input changes, output commits, and activity detection;
//! - External probe feeding and readback via [`AxiVioHandle`].

use pl_native::RegisterBlock;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Core Register Offsets
const VIO_CR_OFFSET: u64 = 0x000;
const VIO_SR_OFFSET: u64 = 0x004;
const VIO_ISR_OFFSET: u64 = 0x008;
const VIO_IER_OFFSET: u64 = 0x00C;

const VIO_PROBE_OUT0_VAL_OFFSET: u64 = 0x010;
const VIO_PROBE_OUT0_STAGING_OFFSET: u64 = 0x014;
const VIO_PROBE_OUT1_VAL_OFFSET: u64 = 0x018;
const VIO_PROBE_OUT1_STAGING_OFFSET: u64 = 0x01C;

const VIO_PROBE_IN0_VAL_OFFSET: u64 = 0x020;
const VIO_PROBE_IN0_LIVE_OFFSET: u64 = 0x024;
const VIO_PROBE_IN1_VAL_OFFSET: u64 = 0x028;
const VIO_PROBE_IN1_LIVE_OFFSET: u64 = 0x02C;

const VIO_PROBE_IN_ACT_OFFSET: u64 = 0x030;
const VIO_PROBE_OUT0_INIT_OFFSET: u64 = 0x034;
const VIO_PROBE_OUT1_INIT_OFFSET: u64 = 0x038;
const VIO_VER_OFFSET: u64 = 0x03C;

// Control Register (CR) Bit Masks
const CR_RESET: u32 = 1 << 0;
const CR_UPDATE_OUT: u32 = 1 << 1;
const CR_LATCH_IN: u32 = 1 << 2;
const CR_AUTO_UPDATE: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;
const CR_STIM_EN: u32 = 1 << 5;

// Status Register (SR) Bit Masks
const SR_OUT_UPDATED: u32 = 1 << 0;
const SR_IN_LATCHED: u32 = 1 << 1;
const SR_ACTIVITY_DET: u32 = 1 << 2;

// Interrupt Status & Enable Masks (ISR / IER)
const IRQ_IN_CHANGE: u32 = 1 << 0;
const IRQ_OUT_ACK: u32 = 1 << 1;
const IRQ_ACTIVITY: u32 = 1 << 2;

// Core Version (vio 3.0)
const VIO_VERSION_3_0: u32 = 0x0300_0000;

/// Static configuration for a Virtual Input/Output core instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VioConfig {
    /// Number of input probes (0..8, default 2).
    pub num_probe_in: u32,
    /// Number of output probes (0..8, default 2).
    pub num_probe_out: u32,
    /// Width of probe in 0 in bits (1..32, default 32).
    pub probe_in0_width: u32,
    /// Width of probe in 1 in bits (1..32, default 32).
    pub probe_in1_width: u32,
    /// Width of probe out 0 in bits (1..32, default 32).
    pub probe_out0_width: u32,
    /// Width of probe out 1 in bits (1..32, default 32).
    pub probe_out1_width: u32,
    /// Initial power-on/reset value of probe out 0.
    pub probe_out0_init: u32,
    /// Initial power-on/reset value of probe out 1.
    pub probe_out1_init: u32,
    /// Activity detection enabled.
    pub en_activity: bool,
    /// Nominal core clock frequency in Hertz.
    pub clk_freq_hz: u64,
    /// Address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for VioConfig {
    fn default() -> Self {
        Self {
            num_probe_in: 2,
            num_probe_out: 2,
            probe_in0_width: 32,
            probe_in1_width: 32,
            probe_out0_width: 32,
            probe_out1_width: 32,
            probe_out0_init: 0,
            probe_out1_init: 0,
            en_activity: true,
            clk_freq_hz: 100_000_000,
            window: 0x10000,
        }
    }
}

impl VioConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = VioConfig {
            window: window.max(0x1000),
            ..VioConfig::default()
        };

        if let Some(v) = params.get("C_NUM_PROBE_IN") {
            if let Ok(n) = v.trim().parse::<u32>() {
                cfg.num_probe_in = n.min(8);
            }
        }
        if let Some(v) = params.get("C_NUM_PROBE_OUT") {
            if let Ok(n) = v.trim().parse::<u32>() {
                cfg.num_probe_out = n.min(8);
            }
        }
        if let Some(v) = params.get("C_PROBE_IN0_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (1..=32).contains(&n) {
                    cfg.probe_in0_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_PROBE_IN1_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (1..=32).contains(&n) {
                    cfg.probe_in1_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_PROBE_OUT0_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (1..=32).contains(&n) {
                    cfg.probe_out0_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_PROBE_OUT1_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (1..=32).contains(&n) {
                    cfg.probe_out1_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_PROBE_OUT0_INIT_VAL") {
            let s = v.trim();
            let parsed = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                s.parse::<u32>().ok()
            };
            if let Some(val) = parsed {
                cfg.probe_out0_init = val;
            }
        }
        if let Some(v) = params.get("C_PROBE_OUT1_INIT_VAL") {
            let s = v.trim();
            let parsed = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                s.parse::<u32>().ok()
            };
            if let Some(val) = parsed {
                cfg.probe_out1_init = val;
            }
        }
        if let Some(v) = params.get("C_EN_PROBE_IN_ACTIVITY") {
            cfg.en_activity = matches!(v.trim(), "1" | "true" | "TRUE");
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

/// Dynamic statistics and performance counters for a VIO core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VioStats {
    /// Number of output commits executed.
    pub outputs_committed: u64,
    /// Number of input latches executed.
    pub inputs_latched: u64,
    /// Number of input toggle/activity events detected.
    pub toggles_detected: u64,
}

/// Internal mutable state of the core.
struct State {
    cfg: VioConfig,
    now: u64,
    clock_acc_ps: u64,

    // Registers
    cr: u32,
    isr: u32,
    ier: u32,

    // Output probes: active and staging
    probe_out_val: [u32; 8],
    probe_out_staging: [u32; 8],

    // Input probes: live, latched, and last seen for toggle detection
    probe_in_live: [u32; 8],
    probe_in_latched: [u32; 8],
    probe_in_last: [u32; 8],
    probe_in_act: u32,

    // Internal stimulus counter for autonomous test mode
    stimulus_counter: u32,

    // Status bits
    sr_out_updated: bool,
    sr_in_latched: bool,

    // Statistics
    stats: VioStats,
}

impl State {
    fn new(cfg: VioConfig) -> Self {
        let mut probe_out_val = [0u32; 8];
        let mut probe_out_staging = [0u32; 8];
        probe_out_val[0] = cfg.probe_out0_init;
        probe_out_val[1] = cfg.probe_out1_init;
        probe_out_staging[0] = cfg.probe_out0_init;
        probe_out_staging[1] = cfg.probe_out1_init;

        Self {
            cfg,
            now: 0,
            clock_acc_ps: 0,
            cr: 0,
            isr: 0,
            ier: 0,
            probe_out_val,
            probe_out_staging,
            probe_in_live: [0u32; 8],
            probe_in_latched: [0u32; 8],
            probe_in_last: [0u32; 8],
            probe_in_act: 0,
            stimulus_counter: 0,
            sr_out_updated: false,
            sr_in_latched: false,
            stats: VioStats::default(),
        }
    }

    fn reset(&mut self) {
        self.cr = 0;
        self.isr = 0;
        self.ier = 0;
        self.probe_out_val = [0u32; 8];
        self.probe_out_staging = [0u32; 8];
        self.probe_out_val[0] = self.cfg.probe_out0_init;
        self.probe_out_val[1] = self.cfg.probe_out1_init;
        self.probe_out_staging[0] = self.cfg.probe_out0_init;
        self.probe_out_staging[1] = self.cfg.probe_out1_init;
        self.probe_in_live = [0u32; 8];
        self.probe_in_latched = [0u32; 8];
        self.probe_in_last = [0u32; 8];
        self.probe_in_act = 0;
        self.stimulus_counter = 0;
        self.sr_out_updated = false;
        self.sr_in_latched = false;
        self.clock_acc_ps = 0;
    }

    fn commit_outputs(&mut self) {
        for i in 0..8 {
            self.probe_out_val[i] = self.probe_out_staging[i];
        }
        self.sr_out_updated = true;
        self.isr |= IRQ_OUT_ACK;
        self.stats.outputs_committed += 1;

        // In autonomous loopback mode, output drives input directly
        if (self.cr & CR_LOOPBACK_EN) != 0 {
            for i in 0..self.cfg.num_probe_in.min(self.cfg.num_probe_out) as usize {
                self.update_probe_in(i, self.probe_out_val[i]);
            }
        }
    }

    fn latch_inputs(&mut self) {
        for i in 0..8 {
            self.probe_in_latched[i] = self.probe_in_live[i];
        }
        self.sr_in_latched = true;
        self.stats.inputs_latched += 1;
    }

    fn update_probe_in(&mut self, idx: usize, val: u32) {
        if idx >= 8 {
            return;
        }
        let mask = if idx == 0 {
            if self.cfg.probe_in0_width >= 32 {
                0xFFFF_FFFF
            } else {
                (1u32 << self.cfg.probe_in0_width) - 1
            }
        } else if idx == 1 {
            if self.cfg.probe_in1_width >= 32 {
                0xFFFF_FFFF
            } else {
                (1u32 << self.cfg.probe_in1_width) - 1
            }
        } else {
            0xFFFF_FFFF
        };

        let masked_val = val & mask;
        let old = self.probe_in_live[idx];
        if old != masked_val {
            self.probe_in_live[idx] = masked_val;
            self.isr |= IRQ_IN_CHANGE;
            if self.cfg.en_activity {
                self.probe_in_act |= 1 << idx;
                self.isr |= IRQ_ACTIVITY;
                self.stats.toggles_detected += 1;
            }
        }
    }

    fn read_status(&self) -> u32 {
        let mut sr = 0u32;
        if self.sr_out_updated {
            sr |= SR_OUT_UPDATED;
        }
        if self.sr_in_latched {
            sr |= SR_IN_LATCHED;
        }
        if self.probe_in_act != 0 {
            sr |= SR_ACTIVITY_DET;
        }
        sr |= (self.cfg.num_probe_in & 0xFF) << 8;
        sr |= (self.cfg.num_probe_out & 0xFF) << 16;
        sr
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
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

        if (self.cr & CR_STIM_EN) != 0 {
            for _ in 0..cycles_to_advance {
                self.stimulus_counter = self.stimulus_counter.wrapping_add(1);
                let s0 = (self.stimulus_counter & 0xFFFF)
                    | ((self.stimulus_counter.wrapping_mul(5)) << 16);
                self.update_probe_in(0, s0);
                if self.cfg.num_probe_in > 1 {
                    self.update_probe_in(1, s0.wrapping_add(0x1000));
                }
            }
        }
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        match offset {
            VIO_CR_OFFSET => self.cr,
            VIO_SR_OFFSET => self.read_status(),
            VIO_ISR_OFFSET => self.isr,
            VIO_IER_OFFSET => self.ier,
            VIO_PROBE_OUT0_VAL_OFFSET => self.probe_out_val[0],
            VIO_PROBE_OUT0_STAGING_OFFSET => self.probe_out_staging[0],
            VIO_PROBE_OUT1_VAL_OFFSET => self.probe_out_val[1],
            VIO_PROBE_OUT1_STAGING_OFFSET => self.probe_out_staging[1],
            VIO_PROBE_IN0_VAL_OFFSET => self.probe_in_latched[0],
            VIO_PROBE_IN0_LIVE_OFFSET => self.probe_in_live[0],
            VIO_PROBE_IN1_VAL_OFFSET => self.probe_in_latched[1],
            VIO_PROBE_IN1_LIVE_OFFSET => self.probe_in_live[1],
            VIO_PROBE_IN_ACT_OFFSET => self.probe_in_act,
            VIO_PROBE_OUT0_INIT_OFFSET => self.cfg.probe_out0_init,
            VIO_PROBE_OUT1_INIT_OFFSET => self.cfg.probe_out1_init,
            VIO_VER_OFFSET => VIO_VERSION_3_0,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            VIO_CR_OFFSET => {
                if (value & CR_RESET) != 0 {
                    self.reset();
                    return;
                }
                self.cr = value & (CR_AUTO_UPDATE | CR_LOOPBACK_EN | CR_STIM_EN);

                if (value & CR_UPDATE_OUT) != 0 || (self.cr & CR_AUTO_UPDATE) != 0 {
                    self.commit_outputs();
                }
                if (value & CR_LATCH_IN) != 0 {
                    self.latch_inputs();
                }
            }
            VIO_ISR_OFFSET => {
                // Write-1-to-clear
                self.isr &= !value;
            }
            VIO_IER_OFFSET => {
                self.ier = value & (IRQ_IN_CHANGE | IRQ_OUT_ACK | IRQ_ACTIVITY);
            }
            VIO_PROBE_OUT0_VAL_OFFSET => {
                self.probe_out_staging[0] = value;
                if (self.cr & CR_AUTO_UPDATE) != 0 {
                    self.commit_outputs();
                }
            }
            VIO_PROBE_OUT0_STAGING_OFFSET => {
                self.probe_out_staging[0] = value;
            }
            VIO_PROBE_OUT1_VAL_OFFSET => {
                self.probe_out_staging[1] = value;
                if (self.cr & CR_AUTO_UPDATE) != 0 {
                    self.commit_outputs();
                }
            }
            VIO_PROBE_OUT1_STAGING_OFFSET => {
                self.probe_out_staging[1] = value;
            }
            VIO_PROBE_IN_ACT_OFFSET => {
                // Write-1-to-clear activity flags
                self.probe_in_act &= !value;
            }
            _ => {}
        }
    }
}

/// Shared control handle for a VIO core instance.
#[derive(Clone)]
pub struct AxiVioHandle {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiVioHandle {
    /// Instance name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reads current output probe value.
    pub fn probe_out(&self, index: usize) -> u32 {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.probe_out_val.get(index).copied().unwrap_or(0)
    }

    /// Drives input probe from external FPGA logic.
    pub fn set_probe_in(&self, index: usize, value: u32) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.update_probe_in(index, value);
    }

    /// Reads current live input probe value.
    pub fn probe_in_live(&self, index: usize) -> u32 {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.probe_in_live.get(index).copied().unwrap_or(0)
    }

    /// Returns static configuration of this core.
    pub fn config(&self) -> VioConfig {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.cfg.clone()
    }

    /// Returns dynamic statistics of this core.
    pub fn stats(&self) -> VioStats {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.stats
    }
}

/// Behavioural register block for the Xilinx Virtual Input/Output core.
pub struct AxiVio {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiVio {
    /// Constructs a new VIO core instance and returns the device and shared handle.
    pub fn new(name: &str, cfg: VioConfig) -> (Self, AxiVioHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = AxiVioHandle {
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

impl RegisterBlock for AxiVio {
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
    fn test_vio_reset_and_initial_values() {
        let cfg = VioConfig {
            probe_out0_init: 0xDEAD_BEEF,
            probe_out1_init: 0xCAFE_BABE,
            ..VioConfig::default()
        };
        let (mut vio, handle) = AxiVio::new("vio_test", cfg);

        let ver = vio.read32(VIO_VER_OFFSET);
        assert_eq!(ver, VIO_VERSION_3_0);

        let out0 = vio.read32(VIO_PROBE_OUT0_VAL_OFFSET);
        let out1 = vio.read32(VIO_PROBE_OUT1_VAL_OFFSET);
        assert_eq!(out0, 0xDEAD_BEEF);
        assert_eq!(out1, 0xCAFE_BABE);
        assert_eq!(handle.probe_out(0), 0xDEAD_BEEF);
        assert_eq!(handle.probe_out(1), 0xCAFE_BABE);

        // Modify staging and reset
        vio.write32(VIO_PROBE_OUT0_STAGING_OFFSET, 0x1234_5678, 0xF);
        vio.write32(VIO_CR_OFFSET, CR_RESET, 0xF);
        let out0_after = vio.read32(VIO_PROBE_OUT0_VAL_OFFSET);
        assert_eq!(out0_after, 0xDEAD_BEEF);
    }

    #[test]
    fn test_vio_staged_output_commit() {
        let (mut vio, handle) = AxiVio::new("vio_test", VioConfig::default());

        // Staging write does not immediately change active output
        vio.write32(VIO_PROBE_OUT0_STAGING_OFFSET, 0x55AA_55AA, 0xF);
        assert_eq!(handle.probe_out(0), 0);
        assert_eq!(vio.read32(VIO_PROBE_OUT0_VAL_OFFSET), 0);

        // Commit outputs via CR_UPDATE_OUT
        vio.write32(VIO_IER_OFFSET, IRQ_OUT_ACK, 0xF);
        vio.write32(VIO_CR_OFFSET, CR_UPDATE_OUT, 0xF);
        assert_eq!(handle.probe_out(0), 0x55AA_55AA);
        assert_eq!(vio.read32(VIO_PROBE_OUT0_VAL_OFFSET), 0x55AA_55AA);
        assert!(vio.irq_level());
        assert_eq!(vio.read32(VIO_ISR_OFFSET) & IRQ_OUT_ACK, IRQ_OUT_ACK);
    }

    #[test]
    fn test_vio_input_latch_and_activity() {
        let (mut vio, handle) = AxiVio::new("vio_test", VioConfig::default());

        vio.write32(VIO_IER_OFFSET, IRQ_IN_CHANGE | IRQ_ACTIVITY, 0xF);

        // Drive external input
        handle.set_probe_in(0, 0x1122_3344);
        assert!(vio.irq_level());
        assert_eq!(vio.read32(VIO_PROBE_IN0_LIVE_OFFSET), 0x1122_3344);
        // Latched value not updated yet
        assert_eq!(vio.read32(VIO_PROBE_IN0_VAL_OFFSET), 0);

        // Activity bit set
        let act = vio.read32(VIO_PROBE_IN_ACT_OFFSET);
        assert_eq!(act & 1, 1);

        // Latch inputs
        vio.write32(VIO_CR_OFFSET, CR_LATCH_IN, 0xF);
        assert_eq!(vio.read32(VIO_PROBE_IN0_VAL_OFFSET), 0x1122_3344);

        // W1C activity
        vio.write32(VIO_PROBE_IN_ACT_OFFSET, 1, 0xF);
        assert_eq!(vio.read32(VIO_PROBE_IN_ACT_OFFSET), 0);
    }

    #[test]
    fn test_vio_loopback_mode() {
        let (mut vio, _) = AxiVio::new("vio_test", VioConfig::default());

        // Enable loopback and auto update
        vio.write32(VIO_CR_OFFSET, CR_LOOPBACK_EN | CR_AUTO_UPDATE, 0xF);

        // Write directly to PROBE_OUT0_VAL
        vio.write32(VIO_PROBE_OUT0_VAL_OFFSET, 0xAABB_CCDD, 0xF);

        // Should automatically loopback to PROBE_IN0_LIVE
        assert_eq!(vio.read32(VIO_PROBE_IN0_LIVE_OFFSET), 0xAABB_CCDD);
    }

    #[test]
    fn test_vio_advance_stimulus() {
        let (mut vio, _) = AxiVio::new("vio_test", VioConfig::default());

        vio.write32(VIO_CR_OFFSET, CR_STIM_EN, 0xF);
        vio.advance(100);
        let s0 = vio.read32(VIO_PROBE_IN0_LIVE_OFFSET);
        assert_ne!(s0, 0);
    }
}
