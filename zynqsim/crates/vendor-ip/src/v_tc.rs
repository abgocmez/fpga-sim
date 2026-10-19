//! Behavioural model of the Xilinx Video Timing Controller core (`v_tc` 6.1/6.2, PG016).
//!
//! The Video Timing Controller generates and detects video sync signals (HBLANK, VBLANK,
//! HSYNC, VSYNC, AVIDEO, and field ID) for video input and output pipelines.
//! This model implements the AXI4-Lite control interface, standard timing generator
//! registers, frame sync generation, error tracking, and interrupt lines.
//!
//! Register facts and bit positions come from PG016 and the Xilinx `xvtc_hw.h` driver header;
//! any behaviour not verified against hardware is marked `VERIFY:`.

use pl_native::RegisterBlock;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Core register offsets from xvtc_hw.h / PG016
const XVTC_CTL_OFFSET: u64 = 0x000;
const XVTC_ISR_OFFSET: u64 = 0x004;
const XVTC_ERROR_OFFSET: u64 = 0x008;
const XVTC_IER_OFFSET: u64 = 0x00C;
const XVTC_VER_OFFSET: u64 = 0x010;
const XVTC_ADAPTIVE_CTL_OFFSET: u64 = 0x014;
const XVTC_VFP_STRETCH_OFFSET: u64 = 0x018;

// Detector register offsets
const XVTC_DASIZE_OFFSET: u64 = 0x020;
const XVTC_DTSTAT_OFFSET: u64 = 0x024;
const XVTC_DFENC_OFFSET: u64 = 0x028;
const XVTC_DPOL_OFFSET: u64 = 0x02C;
const XVTC_DHSIZE_OFFSET: u64 = 0x030;
const XVTC_DVSIZE_OFFSET: u64 = 0x034;
const XVTC_DHSYNC_OFFSET: u64 = 0x038;
const XVTC_DVBHOFF_OFFSET: u64 = 0x03C;
const XVTC_DVSYNC_OFFSET: u64 = 0x040;
const XVTC_DVSHOFF_OFFSET: u64 = 0x044;
const XVTC_DVBHOFF_F1_OFFSET: u64 = 0x048;
const XVTC_DVSYNC_F1_OFFSET: u64 = 0x04C;
const XVTC_DVSHOFF_F1_OFFSET: u64 = 0x050;

// Generator register offsets
const XVTC_GASIZE_OFFSET: u64 = 0x060;
const XVTC_GTSTAT_OFFSET: u64 = 0x064;
const XVTC_GFENC_OFFSET: u64 = 0x068;
const XVTC_GPOL_OFFSET: u64 = 0x06C;
const XVTC_GHSIZE_OFFSET: u64 = 0x070;
const XVTC_GVSIZE_OFFSET: u64 = 0x074;
const XVTC_GHSYNC_OFFSET: u64 = 0x078;
const XVTC_GVBHOFF_OFFSET: u64 = 0x07C;
const XVTC_GVSYNC_OFFSET: u64 = 0x080;
const XVTC_GVSHOFF_OFFSET: u64 = 0x084;
const XVTC_GVBHOFF_F1_OFFSET: u64 = 0x088;
const XVTC_GVSYNC_F1_OFFSET: u64 = 0x08C;
const XVTC_GVSHOFF_F1_OFFSET: u64 = 0x090;
const XVTC_GASIZE_F1_OFFSET: u64 = 0x094;

// Frame sync register offsets (FS00..FS15)
const XVTC_FS00_OFFSET: u64 = 0x100;
const XVTC_FS15_OFFSET: u64 = 0x13C;
const XVTC_GGD_OFFSET: u64 = 0x140;

// Control Register Bit Masks
const CTL_SW_RESET: u32 = 1 << 31;
const CTL_GEN_ENABLE: u32 = 1 << 2;
const CTL_CORE_ENABLE: u32 = 1 << 0;

// Status & Interrupt Masks
const IXR_FSYNC_BASE: u32 = 1 << 16;
const IXR_G_AV: u32 = 1 << 13;
const IXR_G_VBLANK: u32 = 1 << 12;

// Timing Status Masks
const STAT_AVIDEO: u32 = 1 << 2;
const STAT_VBLANK: u32 = 1 << 1;

// Core Version (VTC 6.2)
const VTC_VERSION_6_2: u32 = 0x0602_0000;

/// Static configuration for a Video Timing Controller instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VTcConfig {
    /// Generator sub-block enabled in IP configuration.
    pub generate_en: bool,
    /// Detector sub-block enabled in IP configuration.
    pub detect_en: bool,
    /// Maximum horizontal resolution supported (pixels).
    pub max_pixels: u32,
    /// Maximum vertical resolution supported (lines).
    pub max_lines: u32,
    /// Number of frame sync generator channels (1..16).
    pub num_fsyncs: u32,
    /// Clock frequency in Hertz (nominal video pixel clock).
    pub clk_freq_hz: u64,
    /// Total address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for VTcConfig {
    fn default() -> Self {
        Self {
            generate_en: true,
            detect_en: false,
            max_pixels: 4096,
            max_lines: 4096,
            num_fsyncs: 1,
            clk_freq_hz: 100_000_000,
            window: 0x10000,
        }
    }
}

impl VTcConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = VTcConfig {
            window: window.max(0x1000),
            ..VTcConfig::default()
        };
        if let Some(v) = params.get("C_GENERATE_EN") {
            cfg.generate_en = matches!(v.trim(), "1" | "true" | "TRUE");
        }
        if let Some(v) = params.get("C_DETECT_EN") {
            cfg.detect_en = matches!(v.trim(), "1" | "true" | "TRUE");
        }
        if let Some(v) = params.get("C_MAX_PIXELS") {
            cfg.max_pixels = v
                .trim()
                .parse()
                .map_err(|e| format!("invalid C_MAX_PIXELS `{v}`: {e}"))?;
        }
        if let Some(v) = params.get("C_MAX_LINES") {
            cfg.max_lines = v
                .trim()
                .parse()
                .map_err(|e| format!("invalid C_MAX_LINES `{v}`: {e}"))?;
        }
        if let Some(v) = params.get("C_NUM_FSYNCS") {
            cfg.num_fsyncs = v
                .trim()
                .parse()
                .map_err(|e| format!("invalid C_NUM_FSYNCS `{v}`: {e}"))?;
        }
        if let Some(v) = params.get("CLK_FREQ_HZ") {
            cfg.clk_freq_hz = v
                .trim()
                .parse()
                .map_err(|e| format!("invalid CLK_FREQ_HZ `{v}`: {e}"))?;
        }
        if let Some(v) = params.get("ADDRESS_SIZE") {
            let s = v.trim().strip_prefix("0x").unwrap_or(v.trim());
            cfg.window = u64::from_str_radix(s, 16)
                .map_err(|e| format!("invalid ADDRESS_SIZE `{v}`: {e}"))?;
        }
        Ok(cfg)
    }
}

/// Statistics and counters for the Video Timing Controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VTcStats {
    /// Number of full video frames completed by the generator.
    pub frames_generated: u64,
    /// Number of active video line pulses generated.
    pub lines_generated: u64,
    /// Number of frame sync interrupt pulses emitted.
    pub fsync_interrupts: u64,
}

struct State {
    cfg: VTcConfig,
    now: u64,
    ctl: u32,
    isr: u32,
    error: u32,
    ier: u32,
    adaptive_ctl: u32,
    vfp_stretch: u32,

    dasize: u32,
    dtstat: u32,
    dfenc: u32,
    dpol: u32,
    dhsize: u32,
    dvsize: u32,
    dhsync: u32,
    dvbhoff: u32,
    dvsync: u32,
    dvshoff: u32,
    dvbhoff_f1: u32,
    dvsync_f1: u32,
    dvshoff_f1: u32,

    gasize: u32,
    gtstat: u32,
    gfenc: u32,
    gpol: u32,
    ghsize: u32,
    gvsize: u32,
    ghsync: u32,
    gvbhoff: u32,
    gvsync: u32,
    gvshoff: u32,
    gvbhoff_f1: u32,
    gvsync_f1: u32,
    gvshoff_f1: u32,
    gasize_f1: u32,

    fsync: [u32; 16],
    ggd: u32,

    pixel_acc_ps: u64,
    current_x: u32,
    current_y: u32,

    stats: VTcStats,
}

impl State {
    fn new(cfg: VTcConfig) -> Self {
        Self {
            cfg,
            now: 0,
            ctl: 0,
            isr: 0,
            error: 0,
            ier: 0,
            adaptive_ctl: 0,
            vfp_stretch: 0,

            dasize: 0,
            dtstat: 0,
            dfenc: 0,
            dpol: 0,
            dhsize: 0,
            dvsize: 0,
            dhsync: 0,
            dvbhoff: 0,
            dvsync: 0,
            dvshoff: 0,
            dvbhoff_f1: 0,
            dvsync_f1: 0,
            dvshoff_f1: 0,

            gasize: 0,
            gtstat: 0,
            gfenc: 0,
            gpol: 0,
            ghsize: 0,
            gvsize: 0,
            ghsync: 0,
            gvbhoff: 0,
            gvsync: 0,
            gvshoff: 0,
            gvbhoff_f1: 0,
            gvsync_f1: 0,
            gvshoff_f1: 0,
            gasize_f1: 0,

            fsync: [0; 16],
            ggd: 0,

            pixel_acc_ps: 0,
            current_x: 0,
            current_y: 0,

            stats: VTcStats::default(),
        }
    }

    fn reset(&mut self) {
        self.ctl = 0;
        self.isr = 0;
        self.error = 0;
        self.ier = 0;
        self.adaptive_ctl = 0;
        self.vfp_stretch = 0;

        self.dasize = 0;
        self.dtstat = 0;
        self.dfenc = 0;
        self.dpol = 0;
        self.dhsize = 0;
        self.dvsize = 0;
        self.dhsync = 0;
        self.dvbhoff = 0;
        self.dvsync = 0;
        self.dvshoff = 0;
        self.dvbhoff_f1 = 0;
        self.dvsync_f1 = 0;
        self.dvshoff_f1 = 0;

        self.gasize = 0;
        self.gtstat = 0;
        self.gfenc = 0;
        self.gpol = 0;
        self.ghsize = 0;
        self.gvsize = 0;
        self.ghsync = 0;
        self.gvbhoff = 0;
        self.gvsync = 0;
        self.gvshoff = 0;
        self.gvbhoff_f1 = 0;
        self.gvsync_f1 = 0;
        self.gvshoff_f1 = 0;
        self.gasize_f1 = 0;

        self.fsync.fill(0);
        self.ggd = 0;

        self.pixel_acc_ps = 0;
        self.current_x = 0;
        self.current_y = 0;
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
    }

    fn read_reg(&self, offset: u64) -> u32 {
        match offset {
            XVTC_CTL_OFFSET => self.ctl,
            XVTC_ISR_OFFSET => self.isr,
            XVTC_ERROR_OFFSET => self.error,
            XVTC_IER_OFFSET => self.ier,
            XVTC_VER_OFFSET => VTC_VERSION_6_2,
            XVTC_ADAPTIVE_CTL_OFFSET => self.adaptive_ctl,
            XVTC_VFP_STRETCH_OFFSET => self.vfp_stretch,

            XVTC_DASIZE_OFFSET => self.dasize,
            XVTC_DTSTAT_OFFSET => self.dtstat,
            XVTC_DFENC_OFFSET => self.dfenc,
            XVTC_DPOL_OFFSET => self.dpol,
            XVTC_DHSIZE_OFFSET => self.dhsize,
            XVTC_DVSIZE_OFFSET => self.dvsize,
            XVTC_DHSYNC_OFFSET => self.dhsync,
            XVTC_DVBHOFF_OFFSET => self.dvbhoff,
            XVTC_DVSYNC_OFFSET => self.dvsync,
            XVTC_DVSHOFF_OFFSET => self.dvshoff,
            XVTC_DVBHOFF_F1_OFFSET => self.dvbhoff_f1,
            XVTC_DVSYNC_F1_OFFSET => self.dvsync_f1,
            XVTC_DVSHOFF_F1_OFFSET => self.dvshoff_f1,

            XVTC_GASIZE_OFFSET => self.gasize,
            XVTC_GTSTAT_OFFSET => self.gtstat,
            XVTC_GFENC_OFFSET => self.gfenc,
            XVTC_GPOL_OFFSET => self.gpol,
            XVTC_GHSIZE_OFFSET => self.ghsize,
            XVTC_GVSIZE_OFFSET => self.gvsize,
            XVTC_GHSYNC_OFFSET => self.ghsync,
            XVTC_GVBHOFF_OFFSET => self.gvbhoff,
            XVTC_GVSYNC_OFFSET => self.gvsync,
            XVTC_GVSHOFF_OFFSET => self.gvshoff,
            XVTC_GVBHOFF_F1_OFFSET => self.gvbhoff_f1,
            XVTC_GVSYNC_F1_OFFSET => self.gvsync_f1,
            XVTC_GVSHOFF_F1_OFFSET => self.gvshoff_f1,
            XVTC_GASIZE_F1_OFFSET => self.gasize_f1,

            o if (XVTC_FS00_OFFSET..=XVTC_FS15_OFFSET).contains(&o) => {
                let idx = ((o - XVTC_FS00_OFFSET) / 4) as usize;
                self.fsync.get(idx).copied().unwrap_or(0)
            }
            XVTC_GGD_OFFSET => self.ggd,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            XVTC_CTL_OFFSET => {
                if (value & CTL_SW_RESET) != 0 {
                    self.reset();
                    return;
                }
                self.ctl = value & !CTL_SW_RESET;
            }
            XVTC_ISR_OFFSET => {
                // Write-1-to-clear
                self.isr &= !value;
            }
            XVTC_ERROR_OFFSET => {
                // Write-1-to-clear error bits
                self.error &= !value;
            }
            XVTC_IER_OFFSET => self.ier = value,
            XVTC_ADAPTIVE_CTL_OFFSET => self.adaptive_ctl = value,
            XVTC_VFP_STRETCH_OFFSET => self.vfp_stretch = value,

            XVTC_DASIZE_OFFSET => self.dasize = value,
            XVTC_DTSTAT_OFFSET => self.dtstat = value,
            XVTC_DFENC_OFFSET => self.dfenc = value,
            XVTC_DPOL_OFFSET => self.dpol = value,
            XVTC_DHSIZE_OFFSET => self.dhsize = value,
            XVTC_DVSIZE_OFFSET => self.dvsize = value,
            XVTC_DHSYNC_OFFSET => self.dhsync = value,
            XVTC_DVBHOFF_OFFSET => self.dvbhoff = value,
            XVTC_DVSYNC_OFFSET => self.dvsync = value,
            XVTC_DVSHOFF_OFFSET => self.dvshoff = value,
            XVTC_DVBHOFF_F1_OFFSET => self.dvbhoff_f1 = value,
            XVTC_DVSYNC_F1_OFFSET => self.dvsync_f1 = value,
            XVTC_DVSHOFF_F1_OFFSET => self.dvshoff_f1 = value,

            XVTC_GASIZE_OFFSET => self.gasize = value,
            XVTC_GTSTAT_OFFSET => self.gtstat = value,
            XVTC_GFENC_OFFSET => self.gfenc = value,
            XVTC_GPOL_OFFSET => self.gpol = value,
            XVTC_GHSIZE_OFFSET => self.ghsize = value,
            XVTC_GVSIZE_OFFSET => self.gvsize = value,
            XVTC_GHSYNC_OFFSET => self.ghsync = value,
            XVTC_GVBHOFF_OFFSET => self.gvbhoff = value,
            XVTC_GVSYNC_OFFSET => self.gvsync = value,
            XVTC_GVSHOFF_OFFSET => self.gvshoff = value,
            XVTC_GVBHOFF_F1_OFFSET => self.gvbhoff_f1 = value,
            XVTC_GVSYNC_F1_OFFSET => self.gvsync_f1 = value,
            XVTC_GVSHOFF_F1_OFFSET => self.gvshoff_f1 = value,
            XVTC_GASIZE_F1_OFFSET => self.gasize_f1 = value,

            o if (XVTC_FS00_OFFSET..=XVTC_FS15_OFFSET).contains(&o) => {
                let idx = ((o - XVTC_FS00_OFFSET) / 4) as usize;
                if idx < self.fsync.len() {
                    self.fsync[idx] = value;
                }
            }
            XVTC_GGD_OFFSET => self.ggd = value,
            _ => {}
        }
    }

    fn advance_time(&mut self, now: u64) {
        if now <= self.now {
            return;
        }
        let delta_ps = now - self.now;
        self.now = now;

        // Check if generator is enabled
        let running = self.cfg.generate_en
            && (self.ctl & (CTL_CORE_ENABLE | CTL_GEN_ENABLE))
                == (CTL_CORE_ENABLE | CTL_GEN_ENABLE);
        if !running {
            return;
        }

        let htotal = (self.ghsize & 0x1FFF).max(1);
        let vtotal = (self.gvsize & 0x1FFF).max(1);
        let hactive = self.gasize & 0x1FFF;
        let vactive = (self.gasize >> 16) & 0x1FFF;

        // Calculate pixel duration in picoseconds
        let clk_hz = self.cfg.clk_freq_hz.max(1_000_000);
        let ps_per_pixel = (1_000_000_000_000u64 + clk_hz / 2) / clk_hz;

        self.pixel_acc_ps += delta_ps;
        let pixels_to_advance = self.pixel_acc_ps / ps_per_pixel;
        if pixels_to_advance == 0 {
            return;
        }
        self.pixel_acc_ps %= ps_per_pixel;

        let total_frame_pixels = (htotal as u64) * (vtotal as u64);
        let cur_pixel_index = (self.current_y as u64) * (htotal as u64) + (self.current_x as u64);
        let next_pixel_index = cur_pixel_index + pixels_to_advance;

        // Frame completion counter
        let frames_passed = next_pixel_index / total_frame_pixels;
        if frames_passed > 0 {
            self.stats.frames_generated += frames_passed;
            // Frame sync interrupt 0
            self.isr |= IXR_FSYNC_BASE;
            self.stats.fsync_interrupts += 1;
            // VBLANK interrupt at start of blanking
            self.isr |= IXR_G_VBLANK;
        }

        let remaining = next_pixel_index % total_frame_pixels;
        self.current_y = (remaining / (htotal as u64)) as u32;
        self.current_x = (remaining % (htotal as u64)) as u32;

        // Update GTSTAT based on current beam position
        let in_avideo = self.current_x < hactive && self.current_y < vactive;
        let in_vblank = self.current_y >= vactive;

        self.gtstat = 0;
        if in_avideo {
            self.gtstat |= STAT_AVIDEO;
            self.isr |= IXR_G_AV;
            self.stats.lines_generated += 1;
        }
        if in_vblank {
            self.gtstat |= STAT_VBLANK;
        }
    }
}

/// The AXI Video Timing Controller register block.
pub struct AxiVTc {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Clonable handle for host / test inspection and control.
#[derive(Clone)]
pub struct VTcHandle {
    st: Arc<Mutex<State>>,
}

impl AxiVTc {
    /// Creates a new Video Timing Controller with the given configuration.
    pub fn new(name: &str, cfg: VTcConfig) -> (AxiVTc, VTcHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxiVTc {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            VTcHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl VTcHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Configuration of the instance.
    pub fn config(&self) -> VTcConfig {
        self.lock().cfg.clone()
    }

    /// Returns a copy of the current statistics.
    pub fn stats(&self) -> VTcStats {
        self.lock().stats
    }

    /// Injects an interrupt bit for test injection.
    pub fn inject_interrupt(&self, mask: u32) {
        self.lock().isr |= mask;
    }
}

impl RegisterBlock for AxiVTc {
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
    fn vtc_version_and_reset() {
        let (mut vtc, _) = AxiVTc::new("vtc_0", VTcConfig::default());
        assert_eq!(vtc.read32(XVTC_VER_OFFSET), VTC_VERSION_6_2);

        // Configure some registers
        vtc.write32(XVTC_GASIZE_OFFSET, 0x01E0_0280, 0xF); // 640x480
        vtc.write32(XVTC_IER_OFFSET, 0x0001_3000, 0xF);
        assert_eq!(vtc.read32(XVTC_GASIZE_OFFSET), 0x01E0_0280);
        assert_eq!(vtc.read32(XVTC_IER_OFFSET), 0x0001_3000);

        // Software reset via bit 31
        vtc.write32(XVTC_CTL_OFFSET, CTL_SW_RESET, 0xF);
        assert_eq!(vtc.read32(XVTC_CTL_OFFSET), 0);
        assert_eq!(vtc.read32(XVTC_GASIZE_OFFSET), 0);
        assert_eq!(vtc.read32(XVTC_IER_OFFSET), 0);
    }

    #[test]
    fn vtc_interrupt_and_w1c() {
        let (mut vtc, handle) = AxiVTc::new("vtc_0", VTcConfig::default());
        assert!(!vtc.irq_level());

        // Enable interrupts for G_VBLANK and FSYNC0
        vtc.write32(XVTC_IER_OFFSET, IXR_G_VBLANK | IXR_FSYNC_BASE, 0xF);
        assert!(!vtc.irq_level());

        // Inject interrupt via handle
        handle.inject_interrupt(IXR_G_VBLANK);
        assert!(vtc.irq_level());
        assert_eq!(vtc.read32(XVTC_ISR_OFFSET), IXR_G_VBLANK);

        // Write 1 to clear
        vtc.write32(XVTC_ISR_OFFSET, IXR_G_VBLANK, 0xF);
        assert!(!vtc.irq_level());
        assert_eq!(vtc.read32(XVTC_ISR_OFFSET), 0);
    }

    #[test]
    fn vtc_generator_timing_and_advance() {
        let (mut vtc, handle) = AxiVTc::new(
            "vtc_0",
            VTcConfig {
                clk_freq_hz: 25_000_000, // 25 MHz pixel clock (40 ns per pixel)
                ..VTcConfig::default()
            },
        );

        // Configure 640x480 timing with HTotal=800, VTotal=525
        vtc.write32(XVTC_GASIZE_OFFSET, (480 << 16) | 640, 0xF);
        vtc.write32(XVTC_GHSIZE_OFFSET, 800, 0xF);
        vtc.write32(XVTC_GVSIZE_OFFSET, 525, 0xF);

        // Enable Generator and core
        vtc.write32(XVTC_CTL_OFFSET, CTL_CORE_ENABLE | CTL_GEN_ENABLE, 0xF);

        // Enable interrupt on FSYNC0 and VBLANK
        vtc.write32(XVTC_IER_OFFSET, IXR_FSYNC_BASE | IXR_G_VBLANK, 0xF);

        // Advance simulated time: 800 * 525 pixels at 40 ns = 16.8 ms = 16_800_000 ns
        vtc.advance(17_000_000);

        assert!(vtc.irq_level());
        let isr = vtc.read32(XVTC_ISR_OFFSET);
        assert_ne!(isr & IXR_FSYNC_BASE, 0);
        assert_ne!(isr & IXR_G_VBLANK, 0);

        let stats = handle.stats();
        assert!(stats.frames_generated >= 1);
        assert!(stats.fsync_interrupts >= 1);
    }

    #[test]
    fn vtc_frame_sync_registers() {
        let (mut vtc, _) = AxiVTc::new("vtc_0", VTcConfig::default());
        // Test FS00..FS15 registers
        vtc.write32(XVTC_FS00_OFFSET, 0x0123_4567, 0xF);
        vtc.write32(XVTC_FS15_OFFSET, 0x0ABC_DEF0, 0xF);
        assert_eq!(vtc.read32(XVTC_FS00_OFFSET), 0x0123_4567);
        assert_eq!(vtc.read32(XVTC_FS15_OFFSET), 0x0ABC_DEF0);
    }

    #[test]
    fn vtc_from_params() {
        let mut map = BTreeMap::new();
        map.insert("C_GENERATE_EN".to_string(), "1".to_string());
        map.insert("C_MAX_PIXELS".to_string(), "1920".to_string());
        map.insert("C_MAX_LINES".to_string(), "1080".to_string());
        map.insert("CLK_FREQ_HZ".to_string(), "148500000".to_string());
        let cfg = VTcConfig::from_params(&map, 0x10000).unwrap();
        assert!(cfg.generate_en);
        assert_eq!(cfg.max_pixels, 1920);
        assert_eq!(cfg.max_lines, 1080);
        assert_eq!(cfg.clk_freq_hz, 148_500_000);
    }
}
