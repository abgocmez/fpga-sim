//! Behavioural model of the Xilinx AXI4-Stream to Video Out core (`v_axi4s_vid_out` 4.0, PG044).
//!
//! The AXI4-Stream to Video Out core bridges incoming AXI4-Stream video data (`s_axis_video`)
//! to a native parallel video interface (`vid_io_out`). It synchronizes video data with timing
//! signals from a Video Timing Controller (`v_tc`), aligning frame and line boundaries
//! via `TUSER` (Start of Frame) and `TLAST` (End of Line).
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Internal asynchronous/synchronous FIFO with configurable depth and watermarks;
//! - Alignment state machine (Unsynchronized -> Coarse Align -> Fine Align -> Locked);
//! - Underflow and overflow detection with write-1-to-clear interrupt status (`ISR`);
//! - Video beam progression in simulated time based on clock frequency;
//! - Autonomous test pattern generation (`CR_PATTERN_EN`) for standalone verification;
//! - Host/model inspection and `MediaEvent` emission for GUI/video capture.

use pl_api::MediaEvent;
use pl_native::RegisterBlock;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Core Register Offsets
const VIDOUT_CR_OFFSET: u64 = 0x000;
const VIDOUT_SR_OFFSET: u64 = 0x004;
const VIDOUT_ISR_OFFSET: u64 = 0x008;
const VIDOUT_IER_OFFSET: u64 = 0x00C;
const VIDOUT_ASIZE_OFFSET: u64 = 0x010;
const VIDOUT_TSIZE_OFFSET: u64 = 0x014;
const VIDOUT_FIFO_STATUS_OFFSET: u64 = 0x018;
const VIDOUT_FIFO_THRESH_OFFSET: u64 = 0x01C;
const VIDOUT_STREAM_DATA_OFFSET: u64 = 0x020;
const VIDOUT_STREAM_CTRL_OFFSET: u64 = 0x024;
const VIDOUT_LAST_PIXEL_OFFSET: u64 = 0x028;
const VIDOUT_FRAME_CNT_OFFSET: u64 = 0x030;
const VIDOUT_LINE_CNT_OFFSET: u64 = 0x034;
const VIDOUT_PIXEL_CNT_OFFSET: u64 = 0x038;
const VIDOUT_VER_OFFSET: u64 = 0x03C;

// Control Register (CR) Bit Masks
const CR_EN: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 1;
const CR_MODE_MASTER: u32 = 1 << 2;
const CR_PATTERN_EN: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;

// Status Register (SR) Bit Masks
const SR_LOCKED: u32 = 1 << 0;
const SR_UNDERFLOW: u32 = 1 << 1;
const SR_OVERFLOW: u32 = 1 << 2;
const SR_SOF_DET: u32 = 1 << 3;
const SR_EOL_DET: u32 = 1 << 4;
const SR_FIFO_EMPTY: u32 = 1 << 5;
const SR_FIFO_FULL: u32 = 1 << 6;
const SR_AVIDEO: u32 = 1 << 7;
const SR_VBLANK: u32 = 1 << 8;
const SR_HBLANK: u32 = 1 << 9;

// Alignment State Codes (SR[31:16])
const STATUS_CODE_IDLE: u32 = 0x0000;
const STATUS_CODE_UNSYNC: u32 = 0x0001;
const STATUS_CODE_COARSE: u32 = 0x0002;
const STATUS_CODE_FINE: u32 = 0x0004;
const STATUS_CODE_LOCKED: u32 = 0x0008;

// Interrupt Status & Enable Masks (ISR / IER)
const IRQ_LOCK: u32 = 1 << 0;
const IRQ_UNLOCK: u32 = 1 << 1;
const IRQ_UNDERFLOW: u32 = 1 << 2;
const IRQ_OVERFLOW: u32 = 1 << 3;
const IRQ_SOF: u32 = 1 << 4;
const IRQ_EOL: u32 = 1 << 5;
const IRQ_FRAME_DONE: u32 = 1 << 6;

// Core Version (v_axi4s_vid_out 4.0)
const VIDOUT_VERSION_4_0: u32 = 0x0400_0000;

/// Video synchronization state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    Idle,
    Unsynchronized,
    CoarseAlign,
    FineAlign,
    Locked,
}

/// A pixel packet in the incoming AXI4-Stream FIFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelPacket {
    data: u32,
    tuser: bool, // Start of Frame
    tlast: bool, // End of Line
}

/// Static configuration for an AXI4-Stream to Video Out instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VidOutConfig {
    /// Pixels per video clock cycle (1, 2, or 4).
    pub pixels_per_clock: u32,
    /// Video components per pixel (e.g. 3 for RGB/YUV).
    pub components_per_pixel: u32,
    /// Width in bits per video component (8, 10, 12, or 16).
    pub data_width: u32,
    /// FIFO storage depth in pixel words (32..=8192, typically 1024).
    pub fifo_depth: usize,
    /// True if video clock and AXI stream clock are asynchronous.
    pub async_clk: bool,
    /// Number of consecutive stable frames required to achieve lock.
    pub sync_lock_threshold: u32,
    /// Nominal video clock frequency in Hertz.
    pub clk_freq_hz: u64,
    /// Address window size in bytes (minimum 0x1000).
    pub window: u64,
}

impl Default for VidOutConfig {
    fn default() -> Self {
        Self {
            pixels_per_clock: 1,
            components_per_pixel: 3,
            data_width: 8,
            fifo_depth: 1024,
            async_clk: false,
            sync_lock_threshold: 2,
            clk_freq_hz: 100_000_000,
            window: 0x10000,
        }
    }
}

impl VidOutConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = VidOutConfig {
            window: window.max(0x1000),
            ..VidOutConfig::default()
        };

        if let Some(v) = params.get("C_PIXELS_PER_CLOCK") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.pixels_per_clock = n;
                }
            }
        }
        if let Some(v) = params.get("C_COMPONENTS_PER_PIXEL") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.components_per_pixel = n;
                }
            }
        }
        if let Some(v) = params.get("C_S_AXIS_VIDEO_DATA_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.data_width = n;
                }
            }
        }
        if let Some(v) = params.get("C_ADDR_WIDTH") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if (5..=13).contains(&n) {
                    cfg.fifo_depth = 1 << n;
                }
            }
        }
        if let Some(v) = params.get("C_HAS_ASYNC_CLK") {
            cfg.async_clk = matches!(v.trim(), "1" | "true" | "TRUE");
        }
        if let Some(v) = params.get("C_SYNC_LOCK_THRESHOLD") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    cfg.sync_lock_threshold = n;
                }
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

/// Dynamic statistics and performance counters for an AXI4-Stream to Video Out core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VidOutStats {
    /// Number of completed video frames output.
    pub frames_output: u64,
    /// Number of active pixels transmitted to the video output.
    pub pixels_output: u64,
    /// Number of FIFO underflow events encountered.
    pub underflows: u64,
    /// Number of FIFO overflow events encountered.
    pub overflows: u64,
    /// Number of times the video synchronizer entered locked state.
    pub lock_transitions: u64,
}

/// Internal mutable state of the core.
struct State {
    cfg: VidOutConfig,
    now: u64,
    pixel_acc_ps: u64,

    // Registers
    cr: u32,
    isr: u32,
    ier: u32,
    asize: u32,
    tsize: u32,
    fifo_thresh_low: u16,
    fifo_thresh_high: u16,
    stream_data: u32,
    stream_ctrl: u32,
    last_pixel: u32,

    // State machine & tracking
    sync_state: SyncState,
    stable_frames: u32,
    underflow_flag: bool,
    overflow_flag: bool,
    sof_detected: bool,
    eol_detected: bool,

    // Beam position
    current_x: u32,
    current_y: u32,
    frames_count: u64,

    // FIFO & buffers
    fifo: VecDeque<PixelPacket>,
    captured_frames: VecDeque<Vec<u32>>,
    current_frame_pixels: Vec<u32>,

    // Media events
    media_events: Vec<MediaEvent>,
    stats: VidOutStats,
}

impl State {
    fn new(cfg: VidOutConfig) -> Self {
        let mut s = Self {
            cfg,
            now: 0,
            pixel_acc_ps: 0,
            cr: CR_EN,
            isr: 0,
            ier: 0,
            asize: (480 << 16) | 640,
            tsize: (525 << 16) | 800,
            fifo_thresh_low: 32,
            fifo_thresh_high: 960,
            stream_data: 0,
            stream_ctrl: 0,
            last_pixel: 0,
            sync_state: SyncState::Unsynchronized,
            stable_frames: 0,
            underflow_flag: false,
            overflow_flag: false,
            sof_detected: false,
            eol_detected: false,
            current_x: 0,
            current_y: 0,
            frames_count: 0,
            fifo: VecDeque::new(),
            captured_frames: VecDeque::new(),
            current_frame_pixels: Vec::new(),
            media_events: Vec::new(),
            stats: VidOutStats::default(),
        };
        s.update_watermarks();
        s
    }

    fn update_watermarks(&mut self) {
        let depth = self.cfg.fifo_depth as u16;
        self.fifo_thresh_low = (depth / 32).max(1);
        self.fifo_thresh_high = depth - self.fifo_thresh_low;
    }

    fn reset(&mut self) {
        self.cr = CR_EN;
        self.isr = 0;
        self.ier = 0;
        self.asize = (480 << 16) | 640;
        self.tsize = (525 << 16) | 800;
        self.update_watermarks();
        self.stream_data = 0;
        self.stream_ctrl = 0;
        self.last_pixel = 0;
        self.sync_state = SyncState::Unsynchronized;
        self.stable_frames = 0;
        self.underflow_flag = false;
        self.overflow_flag = false;
        self.sof_detected = false;
        self.eol_detected = false;
        self.current_x = 0;
        self.current_y = 0;
        self.fifo.clear();
        self.current_frame_pixels.clear();
        self.pixel_acc_ps = 0;
    }

    fn active_width(&self) -> u32 {
        (self.asize & 0xFFFF).max(1)
    }

    fn active_height(&self) -> u32 {
        ((self.asize >> 16) & 0xFFFF).max(1)
    }

    fn total_width(&self) -> u32 {
        (self.tsize & 0xFFFF).max(self.active_width())
    }

    fn total_height(&self) -> u32 {
        ((self.tsize >> 16) & 0xFFFF).max(self.active_height())
    }

    fn is_locked(&self) -> bool {
        self.sync_state == SyncState::Locked
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
    }

    fn status_code(&self) -> u32 {
        match self.sync_state {
            SyncState::Idle => STATUS_CODE_IDLE,
            SyncState::Unsynchronized => STATUS_CODE_UNSYNC,
            SyncState::CoarseAlign => STATUS_CODE_COARSE,
            SyncState::FineAlign => STATUS_CODE_FINE,
            SyncState::Locked => STATUS_CODE_LOCKED,
        }
    }

    fn read_status(&self) -> u32 {
        let mut sr = 0u32;
        if self.is_locked() {
            sr |= SR_LOCKED;
        }
        if self.underflow_flag {
            sr |= SR_UNDERFLOW;
        }
        if self.overflow_flag {
            sr |= SR_OVERFLOW;
        }
        if self.sof_detected {
            sr |= SR_SOF_DET;
        }
        if self.eol_detected {
            sr |= SR_EOL_DET;
        }
        if self.fifo.is_empty() {
            sr |= SR_FIFO_EMPTY;
        }
        if self.fifo.len() >= self.cfg.fifo_depth {
            sr |= SR_FIFO_FULL;
        }

        let in_active =
            self.current_x < self.active_width() && self.current_y < self.active_height();
        if in_active {
            sr |= SR_AVIDEO;
        }
        if self.current_y >= self.active_height() {
            sr |= SR_VBLANK;
        }
        if self.current_x >= self.active_width() {
            sr |= SR_HBLANK;
        }

        sr |= self.status_code() << 16;
        sr
    }

    fn push_pixel(&mut self, pkt: PixelPacket) {
        if self.fifo.len() >= self.cfg.fifo_depth {
            self.overflow_flag = true;
            self.isr |= IRQ_OVERFLOW;
            self.stats.overflows += 1;
            return;
        }

        if pkt.tuser {
            self.sof_detected = true;
            self.isr |= IRQ_SOF;
            if self.sync_state == SyncState::Unsynchronized {
                self.sync_state = SyncState::CoarseAlign;
            }
        }
        if pkt.tlast {
            self.eol_detected = true;
            self.isr |= IRQ_EOL;
        }

        self.fifo.push_back(pkt);

        // If we were in CoarseAlign and now have buffered pixels, transition to FineAlign
        if self.sync_state == SyncState::CoarseAlign
            && self.fifo.len() >= self.fifo_thresh_low as usize
        {
            self.sync_state = SyncState::FineAlign;
        }
    }

    fn advance_time(&mut self, now: u64) {
        if now <= self.now {
            return;
        }
        let delta_ps = now - self.now;
        self.now = now;

        if (self.cr & CR_EN) == 0 {
            self.sync_state = SyncState::Idle;
            return;
        }

        let clk_hz = self.cfg.clk_freq_hz.max(1_000_000);
        let ps_per_pixel = (1_000_000_000_000u64 + clk_hz / 2) / clk_hz;

        self.pixel_acc_ps += delta_ps;
        let pixels_to_advance = self.pixel_acc_ps / ps_per_pixel;
        if pixels_to_advance == 0 {
            return;
        }
        self.pixel_acc_ps %= ps_per_pixel;

        let hactive = self.active_width();
        let vactive = self.active_height();
        let htotal = self.total_width();
        let vtotal = self.total_height();

        for _ in 0..pixels_to_advance {
            // Pattern generator mode: automatically supply pixel data if enabled
            if (self.cr & CR_PATTERN_EN) != 0 && self.fifo.len() < (self.cfg.fifo_depth / 2) {
                let px = self.current_x;
                let py = self.current_y;
                let r = ((px * 255) / hactive.max(1)) as u8;
                let g = ((py * 255) / vactive.max(1)) as u8;
                let b = r ^ g;
                let val = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
                let is_sof = px == 0 && py == 0;
                let is_eol = px == (hactive - 1);
                self.push_pixel(PixelPacket {
                    data: val,
                    tuser: is_sof,
                    tlast: is_eol,
                });
            }

            let in_active = self.current_x < hactive && self.current_y < vactive;

            if in_active {
                // Active video requires a pixel from the FIFO
                if let Some(pkt) = self.fifo.pop_front() {
                    self.last_pixel = pkt.data;
                    self.stats.pixels_output += 1;
                    if (self.cr & CR_LOOPBACK_EN) != 0 {
                        self.current_frame_pixels.push(pkt.data);
                    }

                    // Check for FineAlign -> Locked transition
                    if self.sync_state == SyncState::FineAlign {
                        self.sync_state = SyncState::Locked;
                        self.isr |= IRQ_LOCK;
                        self.stats.lock_transitions += 1;
                    }
                } else {
                    // FIFO empty during active video: underflow condition
                    self.underflow_flag = true;
                    self.isr |= IRQ_UNDERFLOW;
                    self.stats.underflows += 1;
                    if self.sync_state == SyncState::Locked {
                        self.sync_state = SyncState::Unsynchronized;
                        self.isr |= IRQ_UNLOCK;
                    }
                }
            }

            // Advance beam coordinates
            self.current_x += 1;
            if self.current_x >= htotal {
                self.current_x = 0;
                self.current_y += 1;
                if self.current_y >= vtotal {
                    self.current_y = 0;
                    self.frames_count += 1;
                    self.stats.frames_output += 1;
                    self.isr |= IRQ_FRAME_DONE;

                    // Stable frame tracking for lock
                    if !self.underflow_flag {
                        self.stable_frames += 1;
                        if self.stable_frames >= self.cfg.sync_lock_threshold
                            && self.sync_state != SyncState::Locked
                            && !self.fifo.is_empty()
                        {
                            self.sync_state = SyncState::Locked;
                            self.isr |= IRQ_LOCK;
                            self.stats.lock_transitions += 1;
                        }
                    } else {
                        self.stable_frames = 0;
                    }

                    if (self.cr & CR_LOOPBACK_EN) != 0 && !self.current_frame_pixels.is_empty() {
                        let frame = std::mem::take(&mut self.current_frame_pixels);
                        self.captured_frames.push_back(frame);
                        if self.captured_frames.len() > 16 {
                            self.captured_frames.pop_front();
                        }
                    }

                    // Emit MediaEvent::VideoFrameEnd
                    self.media_events.push(MediaEvent::VideoFrameEnd {
                        frame: (self.frames_count & 0xFFFF_FFFF) as u32,
                        t_ns: now / 1000,
                    });
                }
            }
        }
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        match offset {
            VIDOUT_CR_OFFSET => self.cr,
            VIDOUT_SR_OFFSET => self.read_status(),
            VIDOUT_ISR_OFFSET => self.isr,
            VIDOUT_IER_OFFSET => self.ier,
            VIDOUT_ASIZE_OFFSET => self.asize,
            VIDOUT_TSIZE_OFFSET => self.tsize,
            VIDOUT_FIFO_STATUS_OFFSET => {
                let lvl = (self.fifo.len() as u32) & 0xFFFF;
                let cap = (self.cfg.fifo_depth as u32) & 0xFFFF;
                lvl | (cap << 16)
            }
            VIDOUT_FIFO_THRESH_OFFSET => {
                u32::from(self.fifo_thresh_low) | (u32::from(self.fifo_thresh_high) << 16)
            }
            VIDOUT_STREAM_DATA_OFFSET => self.stream_data,
            VIDOUT_STREAM_CTRL_OFFSET => self.stream_ctrl,
            VIDOUT_LAST_PIXEL_OFFSET => self.last_pixel,
            VIDOUT_FRAME_CNT_OFFSET => (self.frames_count & 0xFFFF_FFFF) as u32,
            VIDOUT_LINE_CNT_OFFSET => self.current_y,
            VIDOUT_PIXEL_CNT_OFFSET => self.current_x,
            VIDOUT_VER_OFFSET => VIDOUT_VERSION_4_0,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            VIDOUT_CR_OFFSET => {
                if (value & CR_RESET) != 0 {
                    self.reset();
                } else {
                    self.cr = value & (CR_EN | CR_MODE_MASTER | CR_PATTERN_EN | CR_LOOPBACK_EN);
                }
            }
            VIDOUT_ISR_OFFSET => {
                // Write-1-to-clear
                self.isr &= !value;
                if (value & IRQ_UNDERFLOW) != 0 {
                    self.underflow_flag = false;
                }
                if (value & IRQ_OVERFLOW) != 0 {
                    self.overflow_flag = false;
                }
                if (value & IRQ_SOF) != 0 {
                    self.sof_detected = false;
                }
                if (value & IRQ_EOL) != 0 {
                    self.eol_detected = false;
                }
            }
            VIDOUT_IER_OFFSET => {
                self.ier = value & 0x7F;
            }
            VIDOUT_ASIZE_OFFSET => {
                self.asize = value;
            }
            VIDOUT_TSIZE_OFFSET => {
                self.tsize = value;
            }
            VIDOUT_FIFO_THRESH_OFFSET => {
                self.fifo_thresh_low = (value & 0xFFFF) as u16;
                self.fifo_thresh_high = ((value >> 16) & 0xFFFF) as u16;
            }
            VIDOUT_STREAM_DATA_OFFSET => {
                self.stream_data = value;
                let tuser = (self.stream_ctrl & 1) != 0;
                let tlast = (self.stream_ctrl & 2) != 0;
                self.push_pixel(PixelPacket {
                    data: value,
                    tuser,
                    tlast,
                });
                // Auto-clear single-shot flags
                self.stream_ctrl &= !3;
            }
            VIDOUT_STREAM_CTRL_OFFSET => {
                self.stream_ctrl = value;
                if (value & 4) != 0 {
                    // Manual push bit
                    let tuser = (value & 1) != 0;
                    let tlast = (value & 2) != 0;
                    self.push_pixel(PixelPacket {
                        data: self.stream_data,
                        tuser,
                        tlast,
                    });
                    self.stream_ctrl &= !7;
                }
            }
            _ => {}
        }
    }
}

/// Shared handle for host inspection, test manipulation, and metrics.
#[derive(Clone)]
pub struct VidOutHandle(Arc<Mutex<State>>);

impl VidOutHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Pushes an external pixel into the video stream FIFO.
    pub fn push_pixel(&self, data: u32, tuser: bool, tlast: bool) {
        self.lock().push_pixel(PixelPacket { data, tuser, tlast });
    }

    /// Takes the oldest captured full frame from loopback storage, if available.
    pub fn take_frame(&self) -> Option<Vec<u32>> {
        self.lock().captured_frames.pop_front()
    }

    /// Returns whether the video synchronizer has achieved lock.
    pub fn is_locked(&self) -> bool {
        self.lock().is_locked()
    }

    /// Returns cumulative statistics.
    pub fn stats(&self) -> VidOutStats {
        self.lock().stats
    }

    /// Drains any pending media events.
    pub fn drain_media(&self) -> Vec<MediaEvent> {
        std::mem::take(&mut self.lock().media_events)
    }
}

/// AXI4-Stream to Video Out core model implementing [`RegisterBlock`].
pub struct AxiVidOut {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiVidOut {
    /// Constructs a new AXI4-Stream to Video Out instance.
    pub fn new(name: &str, cfg: VidOutConfig) -> (Self, VidOutHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = VidOutHandle(Arc::clone(&state));
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

impl RegisterBlock for AxiVidOut {
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
        let (mut dev, handle) = AxiVidOut::new("vid_out_0", VidOutConfig::default());
        assert_eq!(dev.size(), 0x10000);
        assert_eq!(dev.read32(VIDOUT_VER_OFFSET), VIDOUT_VERSION_4_0);
        assert_eq!(dev.read32(VIDOUT_CR_OFFSET), CR_EN);
        let sr = dev.read32(VIDOUT_SR_OFFSET);
        assert_eq!(sr & SR_LOCKED, 0);
        assert!(!handle.is_locked());
    }

    #[test]
    fn software_reset_clears_registers() {
        let (mut dev, _) = AxiVidOut::new("vid_out_0", VidOutConfig::default());
        dev.write32(VIDOUT_ASIZE_OFFSET, 0x01E0_0280, 0xF);
        dev.write32(VIDOUT_IER_OFFSET, 0x1F, 0xF);
        assert_eq!(dev.read32(VIDOUT_IER_OFFSET), 0x1F);

        // Assert software reset
        dev.write32(VIDOUT_CR_OFFSET, CR_RESET, 0xF);
        assert_eq!(dev.read32(VIDOUT_CR_OFFSET), CR_EN);
        assert_eq!(dev.read32(VIDOUT_IER_OFFSET), 0);
    }

    #[test]
    fn fifo_pixel_push_and_status() {
        let (mut dev, handle) = AxiVidOut::new("vid_out_0", VidOutConfig::default());
        // Configure stream control for SOF
        dev.write32(VIDOUT_STREAM_CTRL_OFFSET, 1, 0xF); // TUSER = 1
        dev.write32(VIDOUT_STREAM_DATA_OFFSET, 0x00FF_0000, 0xF); // Red pixel

        let status = dev.read32(VIDOUT_SR_OFFSET);
        assert_ne!(status & SR_SOF_DET, 0);

        let fifo_st = dev.read32(VIDOUT_FIFO_STATUS_OFFSET);
        assert_eq!(fifo_st & 0xFFFF, 1); // 1 word in FIFO

        // Push another pixel via handle
        handle.push_pixel(0x0000_FF00, false, true); // Green pixel, EOL
        let fifo_st2 = dev.read32(VIDOUT_FIFO_STATUS_OFFSET);
        assert_eq!(fifo_st2 & 0xFFFF, 2);
    }

    #[test]
    fn lock_synchronization_and_pattern_generation() {
        let cfg = VidOutConfig {
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, handle) = AxiVidOut::new("vid_out_0", cfg);

        // Configure a small 8x4 video frame (HTotal=10, VTotal=5)
        dev.write32(VIDOUT_ASIZE_OFFSET, (4 << 16) | 8, 0xF);
        dev.write32(VIDOUT_TSIZE_OFFSET, (5 << 16) | 10, 0xF);

        // Enable Core, Pattern Generator, and Loopback
        dev.write32(
            VIDOUT_CR_OFFSET,
            CR_EN | CR_PATTERN_EN | CR_LOOPBACK_EN,
            0xF,
        );
        dev.write32(VIDOUT_IER_OFFSET, IRQ_LOCK | IRQ_FRAME_DONE, 0xF);

        // Advance 2 microseconds (200 clock cycles at 100MHz = 4 frames of 50 pixels)
        dev.advance(2000);
        assert!(dev.irq_level());
        assert!(handle.is_locked());

        let sr = dev.read32(VIDOUT_SR_OFFSET);
        assert_ne!(sr & SR_LOCKED, 0);

        let stats = handle.stats();
        assert!(stats.frames_output > 0);
        assert!(stats.pixels_output > 0);
        assert_eq!(stats.underflows, 0);

        // Verify captured frame exists
        let frame = handle.take_frame().expect("captured frame");
        assert_eq!(frame.len(), 32); // 8 * 4 active pixels
    }

    #[test]
    fn underflow_detection_and_isr_w1c() {
        let cfg = VidOutConfig {
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, _) = AxiVidOut::new("vid_out_0", cfg);

        // Configure small frame without pattern generator and empty FIFO
        dev.write32(VIDOUT_ASIZE_OFFSET, (4 << 16) | 8, 0xF);
        dev.write32(VIDOUT_TSIZE_OFFSET, (5 << 16) | 10, 0xF);
        dev.write32(VIDOUT_IER_OFFSET, IRQ_UNDERFLOW, 0xF);

        // Advance into active video -> underflow
        dev.advance(500);
        assert!(dev.irq_level());

        let isr = dev.read32(VIDOUT_ISR_OFFSET);
        assert_ne!(isr & IRQ_UNDERFLOW, 0);

        let sr = dev.read32(VIDOUT_SR_OFFSET);
        assert_ne!(sr & SR_UNDERFLOW, 0);

        // Clear ISR via W1C
        dev.write32(VIDOUT_ISR_OFFSET, IRQ_UNDERFLOW, 0xF);
        assert_eq!(dev.read32(VIDOUT_ISR_OFFSET) & IRQ_UNDERFLOW, 0);
    }
}
