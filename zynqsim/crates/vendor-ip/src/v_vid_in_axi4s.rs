//! Behavioural model of the Xilinx Video In to AXI4-Stream core (`v_vid_in_axi4s` 4.0, PG043).
//!
//! The Video In to AXI4-Stream core converts incoming parallel native video data
//! (clock, video data, active video, horizontal/vertical blanking and sync) into an outgoing
//! AXI4-Stream interface (`m_axis_video`). It synchronizes video timing, generates line
//! and frame framing delimiters (`TLAST` and `TUSER[0]`), and absorbs clock domain or bus
//! latency variations using an internal FIFO.
//!
//! This model implements:
//! - Full standard configuration and status register bank as a [`pl_native::RegisterBlock`];
//! - Internal asynchronous/synchronous FIFO with configurable depth and watermarks;
//! - Alignment state machine (Unsynchronized -> Coarse Align -> Fine Align -> Locked);
//! - Overflow and underflow detection with write-1-to-clear interrupt status (`ISR`);
//! - Video beam progression in simulated time based on clock frequency;
//! - Autonomous test pattern generation (`CR_PATTERN_EN`) for standalone verification;
//! - Host/model inspection, stream popping, and `MediaEvent` emission for GUI/video capture.

use pl_api::MediaEvent;
use pl_native::RegisterBlock;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

// Core Register Offsets
const VIDIN_CR_OFFSET: u64 = 0x000;
const VIDIN_SR_OFFSET: u64 = 0x004;
const VIDIN_ISR_OFFSET: u64 = 0x008;
const VIDIN_IER_OFFSET: u64 = 0x00C;
const VIDIN_ASIZE_OFFSET: u64 = 0x010;
const VIDIN_TSIZE_OFFSET: u64 = 0x014;
const VIDIN_FIFO_STATUS_OFFSET: u64 = 0x018;
const VIDIN_FIFO_THRESH_OFFSET: u64 = 0x01C;
const VIDIN_STREAM_DATA_OFFSET: u64 = 0x020;
const VIDIN_STREAM_CTRL_OFFSET: u64 = 0x024;
const VIDIN_LAST_PIXEL_OFFSET: u64 = 0x028;
const VIDIN_FRAME_CNT_OFFSET: u64 = 0x030;
const VIDIN_LINE_CNT_OFFSET: u64 = 0x034;
const VIDIN_PIXEL_CNT_OFFSET: u64 = 0x038;
const VIDIN_VER_OFFSET: u64 = 0x03C;

// Control Register (CR) Bit Masks
const CR_EN: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 1;
const CR_CAPTURE_MODE: u32 = 1 << 2; // 0 = continuous, 1 = single-frame
const CR_PATTERN_EN: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;

// Status Register (SR) Bit Masks
const SR_LOCKED: u32 = 1 << 0;
const SR_OVERFLOW: u32 = 1 << 1;
const SR_UNDERFLOW: u32 = 1 << 2;
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
const IRQ_OVERFLOW: u32 = 1 << 2;
const IRQ_UNDERFLOW: u32 = 1 << 3;
const IRQ_SOF: u32 = 1 << 4;
const IRQ_EOL: u32 = 1 << 5;
const IRQ_FRAME_DONE: u32 = 1 << 6;

// Core Version (v_vid_in_axi4s 4.0)
const VIDIN_VERSION_4_0: u32 = 0x0400_0000;

/// Video synchronization state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    Idle,
    Unsynchronized,
    CoarseAlign,
    FineAlign,
    Locked,
}

/// A pixel packet on the outgoing AXI4-Stream interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelPacket {
    /// Video pixel payload data.
    pub data: u32,
    /// Start of Frame indicator (`TUSER[0]`).
    pub tuser: bool,
    /// End of Line indicator (`TLAST`).
    pub tlast: bool,
}

/// Static configuration for a Video In to AXI4-Stream instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VidInConfig {
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

impl Default for VidInConfig {
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

impl VidInConfig {
    /// Parses configuration from key-value parameters.
    pub fn from_params(params: &BTreeMap<String, String>, window: u64) -> Result<Self, String> {
        let mut cfg = VidInConfig {
            window: window.max(0x1000),
            ..VidInConfig::default()
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
        if let Some(v) = params.get("C_M_AXIS_VIDEO_DATA_WIDTH") {
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

/// Dynamic statistics and performance counters for a Video In to AXI4-Stream core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VidInStats {
    /// Number of completed video frames captured.
    pub frames_captured: u64,
    /// Number of active pixels received.
    pub pixels_captured: u64,
    /// Number of FIFO underflow events encountered.
    pub underflows: u64,
    /// Number of FIFO overflow events encountered.
    pub overflows: u64,
    /// Number of times the video synchronizer entered locked state.
    pub lock_transitions: u64,
}

/// Internal mutable state of the core.
struct State {
    cfg: VidInConfig,
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
    external_input_pixels: VecDeque<u32>,
    captured_frames: VecDeque<Vec<u32>>,
    current_frame_pixels: Vec<u32>,

    // Media events
    media_events: Vec<MediaEvent>,
    stats: VidInStats,
}

impl State {
    fn new(cfg: VidInConfig) -> Self {
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
            external_input_pixels: VecDeque::new(),
            captured_frames: VecDeque::new(),
            current_frame_pixels: Vec::new(),
            media_events: Vec::new(),
            stats: VidInStats::default(),
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
        self.external_input_pixels.clear();
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
        if self.overflow_flag {
            sr |= SR_OVERFLOW;
        }
        if self.underflow_flag {
            sr |= SR_UNDERFLOW;
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

    fn push_pixel_to_fifo(&mut self, pkt: PixelPacket) {
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

        self.last_pixel = pkt.data;
        self.stats.pixels_captured += 1;

        if (self.cr & CR_LOOPBACK_EN) != 0 {
            self.current_frame_pixels.push(pkt.data);
        }

        if self.fifo.len() >= self.cfg.fifo_depth {
            self.overflow_flag = true;
            self.isr |= IRQ_OVERFLOW;
            self.stats.overflows += 1;
            return;
        }

        self.fifo.push_back(pkt);

        // Advance synchronizer state based on buffering
        if self.sync_state == SyncState::CoarseAlign
            && self.fifo.len() >= self.fifo_thresh_low as usize
        {
            self.sync_state = SyncState::FineAlign;
        }
        if self.sync_state == SyncState::FineAlign && self.stable_frames > 0 {
            self.sync_state = SyncState::Locked;
            self.isr |= IRQ_LOCK;
            self.stats.lock_transitions += 1;
        }
    }

    fn pop_pixel_from_fifo(&mut self) -> Option<PixelPacket> {
        if let Some(pkt) = self.fifo.pop_front() {
            Some(pkt)
        } else {
            self.underflow_flag = true;
            self.isr |= IRQ_UNDERFLOW;
            self.stats.underflows += 1;
            None
        }
    }

    fn advance_time(&mut self, now: u64) {
        if now <= self.now {
            return;
        }
        let delta_ps = now - self.now;
        self.now = now;

        if (self.cr & CR_EN) == 0 {
            if self.sync_state == SyncState::Locked {
                self.isr |= IRQ_UNLOCK;
            }
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
            let in_active = self.current_x < hactive && self.current_y < vactive;

            if in_active {
                // Determine pixel value: external input, pattern generator, or default gradient
                let val = if let Some(px) = self.external_input_pixels.pop_front() {
                    px
                } else if (self.cr & CR_PATTERN_EN) != 0 {
                    let px = self.current_x;
                    let py = self.current_y;
                    let r = (((px + 1) * 255) / hactive.max(1)) as u8;
                    let g = (((py + 1) * 255) / vactive.max(1)) as u8;
                    let b = (r ^ g).max(0x10);
                    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
                } else {
                    let px = self.current_x;
                    let py = self.current_y;
                    ((py & 0xFF) << 16) | ((px & 0xFF) << 8) | (((px ^ py) & 0xFF) | 0x10)
                };

                let is_sof = self.current_x == 0 && self.current_y == 0;
                let is_eol = self.current_x == (hactive - 1);

                self.push_pixel_to_fifo(PixelPacket {
                    data: val,
                    tuser: is_sof,
                    tlast: is_eol,
                });
            }

            // Advance beam coordinates
            self.current_x += 1;
            if self.current_x >= htotal {
                self.current_x = 0;
                self.current_y += 1;
                if self.current_y >= vtotal {
                    self.current_y = 0;
                    self.frames_count += 1;
                    self.stats.frames_captured += 1;
                    self.isr |= IRQ_FRAME_DONE;

                    // Stable frame tracking for synchronizer lock to incoming video
                    self.stable_frames += 1;
                    if self.stable_frames >= self.cfg.sync_lock_threshold
                        && self.sync_state != SyncState::Locked
                    {
                        self.sync_state = SyncState::Locked;
                        self.isr |= IRQ_LOCK;
                        self.stats.lock_transitions += 1;
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

                    // Single frame capture mode stops after 1 frame
                    if (self.cr & CR_CAPTURE_MODE) != 0 {
                        self.cr &= !CR_EN;
                        self.sync_state = SyncState::Idle;
                    }
                }
            }
        }
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        match offset {
            VIDIN_CR_OFFSET => self.cr,
            VIDIN_SR_OFFSET => self.read_status(),
            VIDIN_ISR_OFFSET => self.isr,
            VIDIN_IER_OFFSET => self.ier,
            VIDIN_ASIZE_OFFSET => self.asize,
            VIDIN_TSIZE_OFFSET => self.tsize,
            VIDIN_FIFO_STATUS_OFFSET => {
                let lvl = (self.fifo.len() as u32) & 0xFFFF;
                let cap = (self.cfg.fifo_depth as u32) & 0xFFFF;
                lvl | (cap << 16)
            }
            VIDIN_FIFO_THRESH_OFFSET => {
                u32::from(self.fifo_thresh_low) | (u32::from(self.fifo_thresh_high) << 16)
            }
            VIDIN_STREAM_DATA_OFFSET => {
                // Reading returns front pixel data
                if let Some(front) = self.fifo.front() {
                    front.data
                } else {
                    0
                }
            }
            VIDIN_STREAM_CTRL_OFFSET => {
                let mut ctrl = 0u32;
                if let Some(front) = self.fifo.front() {
                    ctrl |= 1; // TVALID
                    if front.tuser {
                        ctrl |= 2; // TUSER
                    }
                    if front.tlast {
                        ctrl |= 4; // TLAST
                    }
                }
                ctrl
            }
            VIDIN_LAST_PIXEL_OFFSET => self.last_pixel,
            VIDIN_FRAME_CNT_OFFSET => (self.frames_count & 0xFFFF_FFFF) as u32,
            VIDIN_LINE_CNT_OFFSET => self.current_y,
            VIDIN_PIXEL_CNT_OFFSET => self.current_x,
            VIDIN_VER_OFFSET => VIDIN_VERSION_4_0,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            VIDIN_CR_OFFSET => {
                if (value & CR_RESET) != 0 {
                    self.reset();
                } else {
                    self.cr = value & (CR_EN | CR_CAPTURE_MODE | CR_PATTERN_EN | CR_LOOPBACK_EN);
                }
            }
            VIDIN_ISR_OFFSET => {
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
            VIDIN_IER_OFFSET => {
                self.ier = value & 0x7F;
            }
            VIDIN_ASIZE_OFFSET => {
                self.asize = value;
            }
            VIDIN_TSIZE_OFFSET => {
                self.tsize = value;
            }
            VIDIN_FIFO_THRESH_OFFSET => {
                self.fifo_thresh_low = (value & 0xFFFF) as u16;
                self.fifo_thresh_high = ((value >> 16) & 0xFFFF) as u16;
            }
            VIDIN_STREAM_CTRL_OFFSET => {
                self.stream_ctrl = value;
                // Bit 3 = POP
                if (value & 8) != 0 {
                    self.pop_pixel_from_fifo();
                }
            }
            _ => {}
        }
    }
}

/// Shared handle for host inspection, test manipulation, and metrics.
#[derive(Clone)]
pub struct VidInHandle(Arc<Mutex<State>>);

impl VidInHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Injects an external raw pixel value into the video input queue.
    pub fn push_input_pixel(&self, data: u32) {
        self.lock().external_input_pixels.push_back(data);
    }

    /// Pops the next outgoing AXI4-Stream packet from the FIFO.
    pub fn pop_packet(&self) -> Option<PixelPacket> {
        self.lock().pop_pixel_from_fifo()
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
    pub fn stats(&self) -> VidInStats {
        self.lock().stats
    }

    /// Drains any pending media events.
    pub fn drain_media(&self) -> Vec<MediaEvent> {
        std::mem::take(&mut self.lock().media_events)
    }
}

/// Video In to AXI4-Stream core model implementing [`RegisterBlock`].
pub struct AxiVidIn {
    name: String,
    state: Arc<Mutex<State>>,
}

impl AxiVidIn {
    /// Constructs a new Video In to AXI4-Stream instance.
    pub fn new(name: &str, cfg: VidInConfig) -> (Self, VidInHandle) {
        let state = Arc::new(Mutex::new(State::new(cfg)));
        let handle = VidInHandle(Arc::clone(&state));
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

impl RegisterBlock for AxiVidIn {
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
        let (mut dev, handle) = AxiVidIn::new("vid_in_0", VidInConfig::default());
        assert_eq!(dev.size(), 0x10000);
        assert_eq!(dev.read32(VIDIN_VER_OFFSET), VIDIN_VERSION_4_0);
        assert_eq!(dev.read32(VIDIN_CR_OFFSET), CR_EN);
        let sr = dev.read32(VIDIN_SR_OFFSET);
        assert_eq!(sr & SR_LOCKED, 0);
        assert!(!handle.is_locked());
    }

    #[test]
    fn software_reset_clears_registers() {
        let (mut dev, _) = AxiVidIn::new("vid_in_0", VidInConfig::default());
        dev.write32(VIDIN_ASIZE_OFFSET, 0x01E0_0280, 0xF);
        dev.write32(VIDIN_IER_OFFSET, 0x1F, 0xF);
        assert_eq!(dev.read32(VIDIN_IER_OFFSET), 0x1F);

        // Assert software reset
        dev.write32(VIDIN_CR_OFFSET, CR_RESET, 0xF);
        assert_eq!(dev.read32(VIDIN_CR_OFFSET), CR_EN);
        assert_eq!(dev.read32(VIDIN_IER_OFFSET), 0);
    }

    #[test]
    fn capture_pattern_locks_and_interrupts() {
        let cfg = VidInConfig {
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, handle) = AxiVidIn::new("vid_in_0", cfg);

        // Configure tiny frame: 8x4 active, 10x5 total (50 pixels per frame)
        // 50 pixels @ 100MHz = 500 ns per frame.
        dev.write32(VIDIN_ASIZE_OFFSET, (4 << 16) | 8, 0xF);
        dev.write32(VIDIN_TSIZE_OFFSET, (5 << 16) | 10, 0xF);
        dev.write32(VIDIN_IER_OFFSET, IRQ_LOCK | IRQ_FRAME_DONE, 0xF);
        dev.write32(VIDIN_CR_OFFSET, CR_EN | CR_PATTERN_EN | CR_LOOPBACK_EN, 0xF);

        // Advance 3 full frames: 1500 ns
        dev.advance(1500);

        let frames = dev.read32(VIDIN_FRAME_CNT_OFFSET);
        assert!(frames >= 2, "Expected >= 2 frames, got {frames}");
        assert!(handle.is_locked(), "Expected synchronizer to be locked");
        let sr = dev.read32(VIDIN_SR_OFFSET);
        assert_ne!(sr & SR_LOCKED, 0);

        assert!(dev.irq_level());
        let events = handle.drain_media();
        assert!(!events.is_empty());
        assert!(matches!(events[0], MediaEvent::VideoFrameEnd { .. }));

        let frame = handle.take_frame().expect("Captured frame in loopback");
        assert_eq!(frame.len(), 32); // 8 * 4 = 32 pixels
    }

    #[test]
    fn stream_pop_via_registers() {
        let cfg = VidInConfig {
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, handle) = AxiVidIn::new("vid_in_0", cfg);

        dev.write32(VIDIN_ASIZE_OFFSET, (4 << 16) | 8, 0xF);
        dev.write32(VIDIN_TSIZE_OFFSET, (5 << 16) | 10, 0xF);
        dev.write32(VIDIN_CR_OFFSET, CR_EN | CR_PATTERN_EN, 0xF);

        // Advance 10 pixels (100 ns)
        dev.advance(100);

        let status = dev.read32(VIDIN_FIFO_STATUS_OFFSET);
        let level = status & 0xFFFF;
        assert!(level > 0, "FIFO should contain pixels, got {level}");

        let ctrl = dev.read32(VIDIN_STREAM_CTRL_OFFSET);
        assert_eq!(ctrl & 1, 1, "TVALID should be asserted");
        assert_eq!(ctrl & 2, 2, "First pixel should have TUSER (SOF)");

        let pixel_data = dev.read32(VIDIN_STREAM_DATA_OFFSET);

        // Pop via register write
        dev.write32(VIDIN_STREAM_CTRL_OFFSET, 8, 0xF); // Bit 3 = POP
        let new_level = dev.read32(VIDIN_FIFO_STATUS_OFFSET) & 0xFFFF;
        assert_eq!(new_level, level - 1);

        // Or pop via handle
        let pkt = handle.pop_packet().expect("packet");
        assert_ne!(pkt.data, 0);
        let _ = pixel_data;
    }

    #[test]
    fn overflow_detection_and_isr_w1c() {
        let cfg = VidInConfig {
            fifo_depth: 32, // Small FIFO to test overflow easily
            clk_freq_hz: 100_000_000,
            ..Default::default()
        };
        let (mut dev, _) = AxiVidIn::new("vid_in_0", cfg);

        dev.write32(VIDIN_ASIZE_OFFSET, (4 << 16) | 32, 0xF);
        dev.write32(VIDIN_TSIZE_OFFSET, (5 << 16) | 40, 0xF);
        dev.write32(VIDIN_IER_OFFSET, IRQ_OVERFLOW, 0xF);
        dev.write32(VIDIN_CR_OFFSET, CR_EN | CR_PATTERN_EN, 0xF);

        // Advance enough pixels to completely overflow 32-deep FIFO
        dev.advance(600); // 60 pixels

        let sr = dev.read32(VIDIN_SR_OFFSET);
        assert_ne!(sr & SR_OVERFLOW, 0, "Expected overflow bit set");
        let isr = dev.read32(VIDIN_ISR_OFFSET);
        assert_ne!(isr & IRQ_OVERFLOW, 0, "Expected IRQ_OVERFLOW set in ISR");
        assert!(dev.irq_level());

        // Clear overflow via write-1-to-clear
        dev.write32(VIDIN_ISR_OFFSET, IRQ_OVERFLOW, 0xF);
        let isr_after = dev.read32(VIDIN_ISR_OFFSET);
        assert_eq!(isr_after & IRQ_OVERFLOW, 0);
        assert!(!dev.irq_level());
    }
}
