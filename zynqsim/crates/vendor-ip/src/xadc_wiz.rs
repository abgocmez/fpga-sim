//! Behavioural model of the Xilinx 7 Series FPGAs and Zynq-7000 XADC Wizard core (`xadc_wiz` 3.3, PG091).
//!
//! The XADC Wizard core provides an AXI4-Lite slave interface to the internal dual
//! 12-bit 1 MSPS Analog-to-Digital Converter block (XADC). It allows software running
//! on the processing system or microcontrollers in the FPGA fabric to monitor on-chip
//! operational parameters such as die temperature, internal supply voltages (VCCINT,
//! VCCAUX, VCCBRAM), dedicated analog inputs (VP/VN), and external auxiliary analog
//! channels (VAUXP/VAUXN).
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Software reset via `SRR` with key `0xA` per PG091;
//! - Direct memory-mapped access to XADC DRP registers at offsets `0x200..=0x3FC`;
//! - High-precision conversions for die temperature (DRP 0x00), VCCINT (DRP 0x01),
//!   VCCAUX (DRP 0x02), VCCBRAM (DRP 0x06), and auxiliary analog channels (DRP 0x10..=0x1F);
//! - Minimum and maximum tracked value registers (DRP 0x20..=0x27);
//! - Programmable alarm threshold registers (DRP 0x50..=0x5F) with automatic alarm status
//!   flag generation (`ALM[7:0]`, `OT`) in `SR` and `AIFR`;
//! - IP interrupt status (`IPISR`, W1C) and enable (`IPIER`) with global interrupt enable (`GIER`);
//! - Continuous conversion sequencer progression advancing with simulated time (`advance`);
//! - External host/sensor injection via [`AxiXadcHandle`].

use pl_native::RegisterBlock;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Standard AXI XADC Wizard Register Offsets (PG091)
const XADC_SRR_OFFSET: u64 = 0x000;
const XADC_SR_OFFSET: u64 = 0x004;
const XADC_AIFR_OFFSET: u64 = 0x008;
const XADC_CONVST_OFFSET: u64 = 0x00C;
const XADC_IPISR_OFFSET: u64 = 0x05C;
const XADC_IPIER_OFFSET: u64 = 0x060;
const XADC_GIER_OFFSET: u64 = 0x068;

// Direct DRP Memory Mapped Window
const XADC_DRP_BASE: u64 = 0x200;
const XADC_DRP_END: u64 = 0x3FC;

// DRP Channel Indices
const DRP_TEMP: usize = 0x00;
const DRP_VCCINT: usize = 0x01;
const DRP_VCCAUX: usize = 0x02;
#[allow(dead_code)]
const DRP_VPVN: usize = 0x03;
#[allow(dead_code)]
const DRP_VREFP: usize = 0x04;
#[allow(dead_code)]
const DRP_VREFN: usize = 0x05;
const DRP_VCCBRAM: usize = 0x06;

// Max/Min tracking DRP Indices
const DRP_MAX_TEMP: usize = 0x20;
const DRP_MAX_VCCINT: usize = 0x21;
const DRP_MAX_VCCAUX: usize = 0x22;
const DRP_MAX_VCCBRAM: usize = 0x23;
const DRP_MIN_TEMP: usize = 0x24;
const DRP_MIN_VCCINT: usize = 0x25;
const DRP_MIN_VCCAUX: usize = 0x26;
const DRP_MIN_VCCBRAM: usize = 0x27;

// Configuration DRP Indices
const DRP_CONF0: usize = 0x40;
const DRP_CONF1: usize = 0x41;
const DRP_CONF2: usize = 0x42;

// Alarm Threshold DRP Indices
const DRP_ALARM_TEMP_UPPER: usize = 0x50;
const DRP_ALARM_VCCINT_UPPER: usize = 0x51;
const DRP_ALARM_VCCAUX_UPPER: usize = 0x52;
const DRP_ALARM_OT_UPPER: usize = 0x53;
const DRP_ALARM_TEMP_LOWER: usize = 0x54;
const DRP_ALARM_VCCINT_LOWER: usize = 0x55;
const DRP_ALARM_VCCAUX_LOWER: usize = 0x56;
const DRP_ALARM_OT_LOWER: usize = 0x57;
const DRP_ALARM_VBRAM_UPPER: usize = 0x58;
const DRP_ALARM_VBRAM_LOWER: usize = 0x5C;

// Software Reset Key
const SRR_RESET_KEY: u32 = 0x0000_000A;

// Status Register (SR) Masks
const SR_DONE: u32 = 1 << 0;
const SR_EOC: u32 = 1 << 1;
const SR_EOS: u32 = 1 << 2;
const SR_BUSY: u32 = 1 << 3;
const SR_OT: u32 = 1 << 24;

// IP Interrupt Status & Enable Masks (IPISR / IPIER)
const IRQ_ALM0: u32 = 1 << 0; // Temp alarm
const IRQ_ALM1: u32 = 1 << 1; // VCCINT alarm
const IRQ_ALM2: u32 = 1 << 2; // VCCAUX alarm
const IRQ_ALM3: u32 = 1 << 3; // VCCBRAM alarm
const IRQ_OT: u32 = 1 << 7; // Over-temperature alarm
const IRQ_EOS: u32 = 1 << 8; // End of sequence
const IRQ_EOC: u32 = 1 << 9; // End of conversion

// Global Interrupt Enable Register Mask
const GIER_ENABLE: u32 = 1 << 31;

/// The UG480 / PG091 transfer function: 12-bit ADC code to degrees Celsius.
pub fn code_to_celsius(code: u16) -> f64 {
    f64::from(code) * 503.975 / 4096.0 - 273.15
}

/// The UG480 / PG091 transfer function: degrees Celsius to 12-bit ADC code.
pub fn celsius_to_code(c: f64) -> u16 {
    ((c + 273.15) * 4096.0 / 503.975).round().clamp(0.0, 4095.0) as u16
}

/// The UG480 / PG091 transfer function: 12-bit ADC code to supply voltage (volts).
pub fn code_to_volts(code: u16) -> f64 {
    f64::from(code) * 3.0 / 4096.0
}

/// The UG480 / PG091 transfer function: supply voltage (volts) to 12-bit ADC code.
pub fn volts_to_code(v: f64) -> u16 {
    (v * 4096.0 / 3.0).round().clamp(0.0, 4095.0) as u16
}

/// Static configuration for an AXI XADC Wizard core instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XadcConfig {
    /// Nominal ADC conversion sampling clock frequency in Hertz.
    pub clk_freq_hz: u64,
    /// Has alarm generation logic enabled.
    pub has_alarm: bool,
    /// Address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for XadcConfig {
    fn default() -> Self {
        Self {
            clk_freq_hz: 100_000_000,
            has_alarm: true,
            window: 0x10000,
        }
    }
}

impl XadcConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = XadcConfig {
            window: window.max(0x1000),
            ..XadcConfig::default()
        };

        if let Some(v) = params.get("CLK_FREQ_HZ") {
            if let Ok(n) = v.trim().parse::<u64>() {
                if n > 0 {
                    cfg.clk_freq_hz = n;
                }
            }
        }
        if let Some(v) = params.get("C_HAS_ALARM") {
            cfg.has_alarm = matches!(v.trim(), "1" | "true" | "TRUE");
        }

        Ok(cfg)
    }
}

/// Dynamic statistics and performance counters for an XADC core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XadcStats {
    /// Total number of ADC conversions completed.
    pub conversions_completed: u64,
    /// Total number of full sequencer passes completed.
    pub sequences_completed: u64,
    /// Number of alarm conditions tripped.
    pub alarms_tripped: u64,
}

/// Internal mutable state of the XADC Wizard core.
struct State {
    cfg: XadcConfig,
    now: u64,
    clock_acc_ps: u64,

    // Core Registers
    ipisr: u32,
    ipier: u32,
    gier: u32,
    aifr: u32,

    // DRP 128-word register array
    drp: [u16; 128],

    // Sequencer state
    seq_channel: usize,
    eoc: bool,
    eos: bool,
    busy: bool,

    // Dynamic stats
    stats: XadcStats,
}

impl State {
    fn new(cfg: XadcConfig) -> Self {
        let mut s = Self {
            cfg,
            now: 0,
            clock_acc_ps: 0,
            ipisr: 0,
            ipier: 0,
            gier: 0,
            aifr: 0,
            drp: [0u16; 128],
            seq_channel: 0,
            eoc: true,
            eos: true,
            busy: false,
            stats: XadcStats::default(),
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.ipisr = 0;
        self.ipier = 0;
        self.gier = 0;
        self.aifr = 0;
        self.drp = [0u16; 128];

        // Standard default analog sensor values
        let temp_code = celsius_to_code(25.0);
        let vccint_code = volts_to_code(1.0);
        let vccaux_code = volts_to_code(1.8);
        let vccbram_code = volts_to_code(1.0);

        self.drp[DRP_TEMP] = temp_code << 4;
        self.drp[DRP_VCCINT] = vccint_code << 4;
        self.drp[DRP_VCCAUX] = vccaux_code << 4;
        self.drp[DRP_VCCBRAM] = vccbram_code << 4;

        // Reset tracking Max/Min
        self.drp[DRP_MAX_TEMP] = temp_code << 4;
        self.drp[DRP_MIN_TEMP] = temp_code << 4;
        self.drp[DRP_MAX_VCCINT] = vccint_code << 4;
        self.drp[DRP_MIN_VCCINT] = vccint_code << 4;
        self.drp[DRP_MAX_VCCAUX] = vccaux_code << 4;
        self.drp[DRP_MIN_VCCAUX] = vccaux_code << 4;
        self.drp[DRP_MAX_VCCBRAM] = vccbram_code << 4;
        self.drp[DRP_MIN_VCCBRAM] = vccbram_code << 4;

        // Default alarm thresholds per UG480
        self.drp[DRP_ALARM_TEMP_UPPER] = celsius_to_code(85.0) << 4;
        self.drp[DRP_ALARM_TEMP_LOWER] = celsius_to_code(0.0) << 4;
        self.drp[DRP_ALARM_OT_UPPER] = celsius_to_code(125.0) << 4;
        self.drp[DRP_ALARM_OT_LOWER] = celsius_to_code(100.0) << 4;
        self.drp[DRP_ALARM_VCCINT_UPPER] = volts_to_code(1.05) << 4;
        self.drp[DRP_ALARM_VCCINT_LOWER] = volts_to_code(0.95) << 4;
        self.drp[DRP_ALARM_VCCAUX_UPPER] = volts_to_code(1.89) << 4;
        self.drp[DRP_ALARM_VCCAUX_LOWER] = volts_to_code(1.71) << 4;
        self.drp[DRP_ALARM_VBRAM_UPPER] = volts_to_code(1.05) << 4;
        self.drp[DRP_ALARM_VBRAM_LOWER] = volts_to_code(0.95) << 4;

        // Default configuration registers
        self.drp[DRP_CONF0] = 0;
        self.drp[DRP_CONF1] = 0;
        self.drp[DRP_CONF2] = 0;

        self.seq_channel = 0;
        self.eoc = true;
        self.eos = true;
        self.busy = false;
        self.clock_acc_ps = 0;
    }

    fn update_alarms(&mut self) {
        if !self.cfg.has_alarm {
            return;
        }

        let temp = self.drp[DRP_TEMP];
        let vccint = self.drp[DRP_VCCINT];
        let vccaux = self.drp[DRP_VCCAUX];
        let vccbram = self.drp[DRP_VCCBRAM];

        let mut aifr = 0u32;
        let mut alarms_tripped = false;

        // Temperature Alarm (ALM0)
        if temp >= self.drp[DRP_ALARM_TEMP_UPPER] || temp <= self.drp[DRP_ALARM_TEMP_LOWER] {
            aifr |= IRQ_ALM0;
            alarms_tripped = true;
        }

        // VCCINT Alarm (ALM1)
        if vccint >= self.drp[DRP_ALARM_VCCINT_UPPER] || vccint <= self.drp[DRP_ALARM_VCCINT_LOWER]
        {
            aifr |= IRQ_ALM1;
            alarms_tripped = true;
        }

        // VCCAUX Alarm (ALM2)
        if vccaux >= self.drp[DRP_ALARM_VCCAUX_UPPER] || vccaux <= self.drp[DRP_ALARM_VCCAUX_LOWER]
        {
            aifr |= IRQ_ALM2;
            alarms_tripped = true;
        }

        // VCCBRAM Alarm (ALM3)
        if vccbram >= self.drp[DRP_ALARM_VBRAM_UPPER] || vccbram <= self.drp[DRP_ALARM_VBRAM_LOWER]
        {
            aifr |= IRQ_ALM3;
            alarms_tripped = true;
        }

        // Over-temperature Alarm (OT)
        if temp >= self.drp[DRP_ALARM_OT_UPPER] {
            aifr |= IRQ_OT;
            alarms_tripped = true;
        }

        self.aifr = aifr;
        if alarms_tripped {
            self.ipisr |= aifr;
            self.stats.alarms_tripped += 1;
        }
    }

    fn read_status(&self) -> u32 {
        let mut sr = 0u32;
        if self.eoc {
            sr |= SR_EOC | SR_DONE;
        }
        if self.eos {
            sr |= SR_EOS;
        }
        if self.busy {
            sr |= SR_BUSY;
        }
        sr |= ((self.seq_channel as u32) & 0x1F) << 8;

        // Alarm status in SR[23:16] and OT in SR[24]
        let alm_flags = self.aifr & 0x0F;
        sr |= alm_flags << 16;
        if (self.aifr & IRQ_OT) != 0 {
            sr |= SR_OT;
        }

        sr
    }

    fn irq(&self) -> bool {
        (self.gier & GIER_ENABLE) != 0 && (self.ipisr & self.ipier) != 0
    }

    fn advance_time(&mut self, now: u64) {
        if now <= self.now {
            return;
        }
        let delta_ps = now - self.now;
        self.now = now;

        // Typical XADC conversion takes 26 ADCCLK cycles (~1 MSPS at 26 MHz ADCCLK)
        // With 100 MHz core clock, a conversion completes every ~1 microsecond (1_000_000 ps)
        let ps_per_conversion = 1_000_000u64;
        self.clock_acc_ps += delta_ps;

        let conversions = self.clock_acc_ps / ps_per_conversion;
        if conversions == 0 {
            return;
        }
        self.clock_acc_ps %= ps_per_conversion;

        for _ in 0..conversions {
            self.seq_channel = (self.seq_channel + 1) % 4; // cycles TEMP, VCCINT, VCCAUX, VCCBRAM
            self.eoc = true;
            self.ipisr |= IRQ_EOC;
            self.stats.conversions_completed += 1;

            if self.seq_channel == 0 {
                self.eos = true;
                self.ipisr |= IRQ_EOS;
                self.stats.sequences_completed += 1;
            }
        }

        self.update_alarms();
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        if (XADC_DRP_BASE..=XADC_DRP_END).contains(&offset) {
            let drp_addr = ((offset - XADC_DRP_BASE) / 4) as usize;
            return self.drp.get(drp_addr).copied().unwrap_or(0) as u32;
        }

        match offset {
            XADC_SRR_OFFSET => 0,
            XADC_SR_OFFSET => self.read_status(),
            XADC_AIFR_OFFSET => self.aifr,
            XADC_CONVST_OFFSET => 0,
            XADC_IPISR_OFFSET => self.ipisr,
            XADC_IPIER_OFFSET => self.ipier,
            XADC_GIER_OFFSET => self.gier,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        if (XADC_DRP_BASE..=XADC_DRP_END).contains(&offset) {
            let drp_addr = ((offset - XADC_DRP_BASE) / 4) as usize;
            if drp_addr < 128 {
                self.drp[drp_addr] = (value & 0xFFFF) as u16;
                self.update_alarms();
            }
            return;
        }

        match offset {
            XADC_SRR_OFFSET => {
                if value == SRR_RESET_KEY {
                    self.reset();
                }
            }
            XADC_CONVST_OFFSET => {
                self.busy = true;
                self.eoc = false;
                self.eos = false;
            }
            XADC_IPISR_OFFSET => {
                // Write-1-to-clear
                self.ipisr &= !value;
            }
            XADC_IPIER_OFFSET => {
                self.ipier = value & 0x3FF;
            }
            XADC_GIER_OFFSET => {
                self.gier = value & GIER_ENABLE;
            }
            _ => {}
        }
    }
}

/// Shared control handle for an XADC core instance.
#[derive(Clone)]
pub struct AxiXadcHandle {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiXadcHandle {
    /// Instance name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sets the simulated on-chip die temperature in degrees Celsius.
    pub fn set_temperature(&self, deg_c: f64) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let code = celsius_to_code(deg_c);
        s.drp[DRP_TEMP] = code << 4;
        let max_cur = s.drp[DRP_MAX_TEMP] >> 4;
        let min_cur = s.drp[DRP_MIN_TEMP] >> 4;
        if code > max_cur {
            s.drp[DRP_MAX_TEMP] = code << 4;
        }
        if code < min_cur {
            s.drp[DRP_MIN_TEMP] = code << 4;
        }
        s.update_alarms();
    }

    /// Sets the simulated VCCINT internal core supply voltage in volts.
    pub fn set_vccint(&self, volts: f64) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let code = volts_to_code(volts);
        s.drp[DRP_VCCINT] = code << 4;
        s.update_alarms();
    }

    /// Sets the simulated VCCAUX auxiliary supply voltage in volts.
    pub fn set_vccaux(&self, volts: f64) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let code = volts_to_code(volts);
        s.drp[DRP_VCCAUX] = code << 4;
        s.update_alarms();
    }

    /// Sets the simulated VCCBRAM block RAM supply voltage in volts.
    pub fn set_vccbram(&self, volts: f64) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let code = volts_to_code(volts);
        s.drp[DRP_VCCBRAM] = code << 4;
        s.update_alarms();
    }

    /// Sets raw 12-bit code on an auxiliary analog input channel (0..15).
    pub fn set_vaux(&self, channel: usize, code: u16) {
        if channel < 16 {
            let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            s.drp[0x10 + channel] = (code & 0x0FFF) << 4;
        }
    }

    /// Returns static configuration of this core.
    pub fn config(&self) -> XadcConfig {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.cfg.clone()
    }

    /// Returns dynamic statistics of this core.
    pub fn stats(&self) -> XadcStats {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.stats
    }
}

/// Behavioural register block for the Xilinx XADC Wizard core.
pub struct AxiXadc {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiXadc {
    /// Constructs a new XADC Wizard instance and returns the device and shared handle.
    pub fn new(name: &str, cfg: XadcConfig) -> (Self, AxiXadcHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = AxiXadcHandle {
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

impl RegisterBlock for AxiXadc {
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
    fn test_xadc_reset_and_drp_defaults() {
        let (mut xadc, handle) = AxiXadc::new("xadc_test", XadcConfig::default());

        let temp_raw = xadc.read32(XADC_DRP_BASE + 4 * DRP_TEMP as u64);
        let temp_c = code_to_celsius((temp_raw >> 4) as u16);
        assert!((temp_c - 25.0).abs() < 1.0, "Expected ~25 C, got {temp_c}");

        let vccint_raw = xadc.read32(XADC_DRP_BASE + 4 * DRP_VCCINT as u64);
        let vccint_v = code_to_volts((vccint_raw >> 4) as u16);
        assert!(
            (vccint_v - 1.0).abs() < 0.05,
            "Expected ~1.0 V, got {vccint_v}"
        );

        // Test software reset key 0xA
        handle.set_temperature(75.0);
        let temp_high = xadc.read32(XADC_DRP_BASE + 4 * DRP_TEMP as u64);
        assert_ne!(temp_high, temp_raw);

        xadc.write32(XADC_SRR_OFFSET, SRR_RESET_KEY, 0xF);
        let temp_after = xadc.read32(XADC_DRP_BASE + 4 * DRP_TEMP as u64);
        assert_eq!(temp_after, temp_raw);
    }

    #[test]
    fn test_xadc_drp_write_read() {
        let (mut xadc, _) = AxiXadc::new("xadc_test", XadcConfig::default());

        // Write user calibration register at DRP 0x40 (DRP_CONF0)
        let conf_offset = XADC_DRP_BASE + 4 * DRP_CONF0 as u64;
        xadc.write32(conf_offset, 0x1234, 0xF);
        let val = xadc.read32(conf_offset);
        assert_eq!(val, 0x1234);
    }

    #[test]
    fn test_xadc_alarm_and_interrupt() {
        let (mut xadc, handle) = AxiXadc::new("xadc_test", XadcConfig::default());

        // Enable global interrupt and ALM0 / OT interrupt
        xadc.write32(XADC_GIER_OFFSET, GIER_ENABLE, 0xF);
        xadc.write32(XADC_IPIER_OFFSET, IRQ_ALM0 | IRQ_OT, 0xF);

        assert!(!xadc.irq_level());

        // Trigger over-temperature (set to 130 C > 125 C threshold)
        handle.set_temperature(130.0);
        assert!(xadc.irq_level());

        let sr = xadc.read32(XADC_SR_OFFSET);
        assert_ne!(sr & SR_OT, 0);

        let ipisr = xadc.read32(XADC_IPISR_OFFSET);
        assert_ne!(ipisr & IRQ_OT, 0);

        // Cool down and clear interrupt
        handle.set_temperature(25.0);
        xadc.write32(XADC_IPISR_OFFSET, IRQ_ALM0 | IRQ_OT, 0xF);
        assert!(!xadc.irq_level());
    }

    #[test]
    fn test_xadc_conversion_progression() {
        let (mut xadc, _) = AxiXadc::new("xadc_test", XadcConfig::default());

        xadc.write32(XADC_GIER_OFFSET, GIER_ENABLE, 0xF);
        xadc.write32(XADC_IPIER_OFFSET, IRQ_EOC | IRQ_EOS, 0xF);

        // Advance simulated time by 5 microseconds (5_000 ns)
        xadc.advance(5_000);
        assert!(xadc.irq_level());

        let ipisr = xadc.read32(XADC_IPISR_OFFSET);
        assert_ne!(ipisr & (IRQ_EOC | IRQ_EOS), 0);
    }
}
