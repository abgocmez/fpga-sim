//! Engine: Xilinx QEMU as the PS; native Rust models or an RTL design in
//! XSim as the PL.
//!
//! Fidelity is **F0 plus instruction-count timing**, and the PL models are
//! transaction level (no clock-cycle timing). Both are stated in the status
//! this engine reports.

use crate::bytelog::ByteLog;
use crate::elf::Symbols;
use crate::engine::Engine;
use crate::error::ApiError;
use crate::netlog::NetLog;
use crate::source::SourceMap;
use crate::types::DeviceStatus;
use crate::types::{
    AudioSpec, BreakKind, BreakpointInfo, Capability, Command, CpuRegs, DebugStatus, DisasmLine,
    Fidelity, Input, MediaStatus, Query, Response, SourceLine, SourceRef, Status, StorageDevice,
    StorageStatus, SymbolInfo, TraceFormat, VideoSpec,
};
use backend_qemu::{BreakKind as GdbKind, QemuConfig, QemuError, QemuMachine, RunEnd, UartMode};
use board_zybo::{BoardDef, BoardState};
use devices::BoardDevice;
use pl_api::media::{SAMPLE_RATE, wav_bytes};
use pl_api::{
    AudioSource, BusResult, MasterAccess, MediaEvent, PlTarget, PsMasterPort, PsPort, VideoSource,
};
use pl_native::{MathIp, NativePl, RegisterBlock};
use sim_core::Ps;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use trace::{TraceEvent, TraceLog};
use vendor_ip::{VendorInstance, VendorPl};

/// Flush the UART and PL accesses into the trace at this virtual-time
/// granularity. Fixed, so the trace is identical however the caller slices
/// its run requests.
const CHUNK_NS: u64 = 100_000;
/// Trace records kept in memory; older ones are dropped (the digest keeps
/// covering them). A firmware polling a register makes thousands per second.
const TRACE_RETAIN: usize = 1_000_000;
/// Journal entries and bytes kept for snapshot/replay; beyond either the
/// journal is dropped and those features report why.
const JOURNAL_MAX_STEPS: usize = 2_000_000;
const JOURNAL_MAX_BYTES: usize = 64 << 20;
/// Real-time limit for one `run_until`.
const WALL_LIMIT: Duration = Duration::from_secs(120);

/// A native PL model to place on a GP port.
#[derive(Clone, Debug)]
pub struct NativeInstance {
    /// Instance name from the project, for diagnostics.
    pub name: String,
    /// Model name (`my_math_ip`).
    pub model: String,
    /// `M_AXI_GP` port, 0 or 1.
    pub gp: u8,
    /// Base address.
    pub base: u64,
    /// Size of the address window; `None` uses the model's default (64 KiB).
    pub size: Option<u64>,
    /// `IRQ_F2P` bit the instance's interrupt output drives.
    pub irq: Option<u32>,
    /// Interrupt outputs by port name with their destination (`IRQ_F2P[n]`, or an interrupt
    /// controller input `axi_intc_0.intr[k]`); when not empty it replaces `irq`.
    pub irqs: Vec<(String, String)>,
    /// IP parameters by Vivado name (`C_GPIO_WIDTH`), plus `CLK_FREQ_HZ`.
    pub params: std::collections::BTreeMap<String, String>,
    /// Board bindings: GPIO interface (`GPIO`, `GPIO2`) to board signal (`board.leds`); the AXI
    /// IIC's `IIC` to a Pmod port.
    pub bindings: std::collections::BTreeMap<String, String>,
}

/// An RTL design to run in XSim as the PL.
#[derive(Clone, Debug)]
pub struct RtlSpec {
    /// Vivado install directory.
    pub vivado_root: PathBuf,
    /// Sources, testbench included.
    pub sources: Vec<PathBuf>,
    /// Top module.
    pub top: String,
    /// Preprocessor defines.
    pub defines: Vec<String>,
    /// The freestanding `libxsim_dpi.a`.
    pub dpi_lib: PathBuf,
    /// Record a waveform database.
    pub waves: bool,
    /// A Vivado simulation export compiled before `sources` (`[rtl] export_dir`/`filelist`).
    pub export: Option<xsim_export::ExportDesign>,
    /// `[rtl] protocol_checkers = "strict"`: a protocol violation is an `error` record
    /// (and fails `zynqsim run`) instead of a `warning`.
    pub protocol_strict: bool,
}

/// Where the contents of the SD card or the QSPI flash come from.
#[derive(Clone)]
pub enum ImageSource {
    /// A raw image file on the host (opened read-only unless the storage is
    /// `writable`).
    File(PathBuf),
    /// An image built in memory at project load from a host folder (SD card)
    /// or file (flash). Each engine writes it into its own run directory, so
    /// guest writes never reach the host and a restart begins from the same
    /// contents.
    Generated {
        /// What it was built from, for the status.
        name: String,
        /// The image.
        image: Arc<Vec<u8>>,
    },
}

impl std::fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageSource::File(p) => write!(f, "File({})", p.display()),
            ImageSource::Generated { name, image } => {
                write!(f, "Generated({name}, {} bytes)", image.len())
            }
        }
    }
}

/// The board's microSD card and QSPI flash.
#[derive(Clone, Debug, Default)]
pub struct StorageSpec {
    /// The card.
    pub sd: Option<ImageSource>,
    /// The 16 MiB QSPI flash contents.
    pub qspi: Option<ImageSource>,
    /// QEMU flash model; [`crate::storage::DEFAULT_QSPI_MODEL`] when `None`.
    pub qspi_model: Option<String>,
    /// Let guest writes reach the host image files.
    pub writable: bool,
}

impl StorageSpec {
    /// True when anything is attached.
    pub fn any(&self) -> bool {
        self.sd.is_some() || self.qspi.is_some()
    }

    /// True when a guest write would change a host file, which makes the
    /// session impossible to re-execute from the same starting point.
    pub fn mutates_host_files(&self) -> bool {
        let file = |s: &Option<ImageSource>| matches!(s, Some(ImageSource::File(_)));
        self.writable && (file(&self.sd) || file(&self.qspi))
    }
}

/// Everything needed to start the QEMU engine.
#[derive(Clone, Debug)]
pub struct QemuSpec {
    /// Xilinx `qemu-system-aarch64`.
    pub qemu: PathBuf,
    /// Co-simulation DTB.
    pub dtb: PathBuf,
    /// Core 0 software.
    pub elf0: PathBuf,
    /// Core 1 software.
    pub elf1: Option<PathBuf>,
    /// Scratch directory for logs.
    pub work_dir: PathBuf,
    /// `-sync-quantum` in nanoseconds.
    pub quantum_ns: u64,
    /// `-icount shift=` value.
    pub icount_shift: u32,
    /// UART1 mode.
    pub uart: UartMode,
    /// Native PL models.
    pub natives: Vec<NativeInstance>,
    /// An RTL design. Native models placed next to it answer in their own windows; every
    /// other access goes to the RTL (ADR-016).
    pub rtl: Option<RtlSpec>,
    /// Board machine name.
    pub board: String,
    /// The board description when it is not looked up by `board` (a custom board file).
    pub board_def: Option<BoardDef>,
    /// Name to report as the PL backend.
    pub pl_name: String,
    /// Attach QEMU's gdb stub (see `[debug]` in `project.toml`).
    pub debug: bool,
    /// Execution breakpoints to plant at start: addresses, ELF symbols or
    /// `file.rs:LINE` (needs DWARF line info in the ELF).
    pub breakpoints: Vec<String>,
    /// Absolute directories searched for the sources of the ELF's line table
    /// (besides its compilation directory), the project directory first.
    pub source_dirs: Vec<PathBuf>,
    /// Devices on Pmod ports: `(port 0..=4 for JA..JE, kind)`.
    pub devices: Vec<(u8, String)>,
    /// Initial physical quantities of those devices: `(port, name, value)`.
    pub device_values: Vec<(u8, String, f64)>,
    /// Attach GEM0 to a simulated LAN (`[network]`), or `None` for no NIC.
    pub net: Option<backend_qemu::net::NetConfig>,
    /// SD card and QSPI flash.
    pub storage: StorageSpec,
    /// Map the PS GPIO, SPI and devcfg blocks (`[ps] extra_blocks`).
    pub ps_extra_blocks: bool,
}

struct BusEvent {
    t_ns: u64,
    port: u8,
    write: bool,
    addr: u64,
    len: u32,
    data: u64,
    ok: bool,
}

/// Wraps a PL target and remembers every PS access for the trace.
struct TracedPl {
    inner: Box<dyn PlTarget + Send>,
    events: Vec<BusEvent>,
}

fn first8(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    u64::from_le_bytes(a)
}

impl PlTarget for TracedPl {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let r = self.inner.read(gp, addr, buf, now_ns, ps);
        self.events.push(BusEvent {
            t_ns: now_ns,
            port: gp,
            write: false,
            addr,
            len: buf.len() as u32,
            data: first8(buf),
            ok: r.is_ok(),
        });
        r
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let r = self.inner.write(gp, addr, data, now_ns, ps);
        self.events.push(BusEvent {
            t_ns: now_ns,
            port: gp,
            write: true,
            addr,
            len: data.len() as u32,
            data: first8(data),
            ok: r.is_ok(),
        });
        r
    }

    fn wire(&mut self, line: u32, level: bool, now_ns: u64) {
        self.inner.wire(line, level, now_ns);
    }

    fn advance(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        self.inner.advance(now_ns, ps);
    }

    fn drain_master_log(&mut self) -> Vec<MasterAccess> {
        self.inner.drain_master_log()
    }

    fn drain_media(&mut self) -> Vec<pl_api::MediaEvent> {
        self.inner.drain_media()
    }

    fn set_media_sources(
        &mut self,
        video: Option<pl_api::VideoSource>,
        audio: Option<pl_api::AudioSource>,
    ) {
        self.inner.set_media_sources(video, audio);
    }

    fn has_media(&self) -> bool {
        self.inner.has_media()
    }

    fn has_board_pins(&self) -> bool {
        self.inner.has_board_pins()
    }

    fn uart_channels(&self) -> Vec<String> {
        self.inner.uart_channels()
    }

    fn drain_uart_tx(&mut self) -> Vec<pl_api::UartByte> {
        self.inner.drain_uart_tx()
    }

    fn uart_rx(&mut self, channel: u8, data: &[u8], now_ns: u64) -> usize {
        self.inner.uart_rx(channel, data, now_ns)
    }

    fn has_pin_groups(&self) -> bool {
        self.inner.has_pin_groups()
    }

    fn drain_pin_events(&mut self) -> Vec<pl_api::PinEvent> {
        self.inner.drain_pin_events()
    }

    fn set_pin_inputs(&mut self, group: u8, levels: u8, now_ns: u64) {
        self.inner.set_pin_inputs(group, levels, now_ns);
    }

    fn set_pin_sink(&mut self, sink: Box<dyn pl_api::PinSink>) {
        self.inner.set_pin_sink(sink);
    }

    fn set_board_inputs(&mut self, switches: u8, buttons: u8, now_ns: u64) {
        self.inner.set_board_inputs(switches, buttons, now_ns);
    }

    fn drain_board_outputs(&mut self) -> Vec<pl_api::BoardOutput> {
        self.inner.drain_board_outputs()
    }

    fn drain_protocol_findings(&mut self) -> Vec<pl_api::ProtocolFinding> {
        self.inner.drain_protocol_findings()
    }

    fn finished(&self) -> bool {
        self.inner.finished()
    }
}

fn master_port_name(p: PsMasterPort) -> &'static str {
    match p {
        PsMasterPort::SAxiGp0 => "S_AXI_GP0",
        PsMasterPort::SAxiGp1 => "S_AXI_GP1",
        PsMasterPort::SAxiHp0 => "S_AXI_HP0",
        PsMasterPort::SAxiHp1 => "S_AXI_HP1",
        PsMasterPort::SAxiHp2 => "S_AXI_HP2",
        PsMasterPort::SAxiHp3 => "S_AXI_HP3",
        PsMasterPort::SAxiAcp => "S_AXI_ACP",
    }
}

/// Builds a native model by name.
fn model(name: &str) -> Option<Box<dyn RegisterBlock>> {
    match name {
        "my_math_ip" => Some(Box::new(MathIp::new())),
        _ => None,
    }
}

/// Names accepted for `[native]` models.
pub const NATIVE_MODELS: &[&str] = &[
    "my_math_ip",
    "axi_gpio",
    "axi_timer",
    "axi_uartlite",
    "axi_dma",
    "axi_iic",
    "axi_bram_ctrl",
    "axi_intc",
];

/// Gives the PL adapter access to the devices on the Pmod ports.
struct BankSink(Arc<Mutex<devices::Bank>>);

impl pl_api::PinSink for BankSink {
    fn pin_change(&mut self, group: u8, t_ns: u64, levels: u8) -> u8 {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pins(group, t_ns, levels)
    }

    fn video_line(&mut self, frame: u32, y: u32, w: u32, h: u32) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .video_line(frame, y, w, h)
    }
}

fn make_device(port: u8, kind: &str) -> Result<Box<dyn BoardDevice>, ApiError> {
    if port > devices::CAMERA_PORT {
        return Err(ApiError::invalid_argument(
            "ports with PL pins are JA..JE (0..=4) and the camera connector CAM (5)",
        ));
    }
    if !devices::fits_port(kind, port) && devices::KINDS.contains(&kind) {
        return Err(ApiError::invalid_argument(if kind == "pcam" {
            "the Pcam plugs into the camera connector CAM (port 5), not a Pmod port".to_string()
        } else {
            format!("`{kind}` is a Pmod module: use a port JA..JE (0..=4)")
        }));
    }
    devices::create(kind).ok_or_else(|| {
        ApiError::invalid_argument(format!(
            "unknown device `{kind}`; available: {}",
            devices::KINDS.join(", ")
        ))
    })
}

/// QEMU's SD model reports a card above 2 GiB as high capacity (block
/// addressing) and anything else as standard capacity (byte addressing).
fn sd_model(bytes: u64) -> String {
    let kind = if bytes > 2 << 30 { "SDHC" } else { "SDSC" };
    format!("sd-card ({kind})")
}

fn check_size(w: u32, h: u32) -> Result<(), ApiError> {
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        Err(ApiError::invalid_argument(format!(
            "video size {w}x{h} is out of range (1..=4096)"
        )))
    } else {
        Ok(())
    }
}

fn map_err(e: QemuError) -> ApiError {
    match e {
        QemuError::MissingFile(p) => {
            ApiError::not_found(format!("file not found: {}", p.display()))
        }
        e @ QemuError::Timeout { .. } => ApiError::io(e.to_string())
            .with_hint("the firmware may be waiting on something that never happens"),
        other => ApiError::io(other.to_string()),
    }
}

/// Seconds of codec DAC audio kept for capture.
const AUDIO_RETAIN_FRAMES: usize = (SAMPLE_RATE as usize) * 10;
/// Incomplete video frames kept while lines arrive.
const PENDING_FRAMES: usize = 3;

/// Audio and video produced by the PL's media models.
#[derive(Default)]
struct Media {
    /// Retained interleaved stereo DAC samples (newest at the back).
    audio: VecDeque<i16>,
    /// Index of the first retained stereo frame.
    audio_first: u64,
    audio_total: u64,
    /// Lines of frames in progress: frame -> (y -> rgb).
    pending: HashMap<u32, HashMap<u32, Vec<u8>>>,
    /// The newest complete frame.
    frame: Option<(u32, u32, u32, Vec<u8>)>,
    frames: u32,
    codec_writes: u32,
}

impl Media {
    fn push_audio(&mut self, first_frame: u64, samples: &[i16]) {
        if self.audio.is_empty() {
            self.audio_first = first_frame;
        }
        self.audio.extend(samples.iter().copied());
        self.audio_total += (samples.len() / 2) as u64;
        while self.audio.len() / 2 > AUDIO_RETAIN_FRAMES {
            self.audio.pop_front();
            self.audio.pop_front();
            self.audio_first += 1;
        }
    }

    /// The retained frames from `from` (clamped to what is retained) on, at
    /// most `max` of them.
    fn slice(&self, from: u64, max: usize) -> AudioSlice {
        let have = (self.audio.len() / 2) as u64;
        let end = self.audio_first + have;
        let start = from.clamp(self.audio_first, end);
        let n = ((end - start) as usize).min(max);
        let skip = (start - self.audio_first) as usize * 2;
        let samples: Vec<i16> = self.audio.iter().skip(skip).take(n * 2).copied().collect();
        AudioSlice {
            first_frame: start,
            frames: n as u64,
            samples,
        }
    }

    fn end_frame(&mut self, frame: u32) {
        let Some(lines) = self.pending.remove(&frame) else {
            return;
        };
        let h = lines.len() as u32;
        // A frame is complete when lines 0..h-1 are all present.
        if h == 0 || !(0..h).all(|y| lines.contains_key(&y)) {
            return;
        }
        let w = (lines[&0].len() / 3) as u32;
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            let l = &lines[&y];
            if l.len() != (w * 3) as usize {
                return; // ragged: not a frame
            }
            rgb.extend_from_slice(l);
        }
        self.frame = Some((w, h, frame, rgb));
        self.frames += 1;
    }

    fn status(&self) -> MediaStatus {
        MediaStatus {
            video_frames: self.frames,
            video_size: self.frame.as_ref().map(|f| (f.0, f.1)),
            audio_frames: self.audio_total,
            audio_peak: self
                .audio
                .iter()
                .map(|&s| i32::from(s).abs())
                .max()
                .unwrap_or(0),
            codec_writes: self.codec_writes,
        }
    }
}

/// A run of retained codec output.
struct AudioSlice {
    first_frame: u64,
    frames: u64,
    samples: Vec<i16>,
}

impl AudioSlice {
    fn into_response(self) -> Response {
        let peak = self
            .samples
            .iter()
            .map(|&s| i32::from(s).abs())
            .max()
            .unwrap_or(0);
        let rms = (self
            .samples
            .iter()
            .map(|&s| f64::from(s).powi(2))
            .sum::<f64>()
            / self.samples.len().max(1) as f64)
            .sqrt();
        Response::Audio {
            sample_rate: SAMPLE_RATE,
            first_frame: self.first_frame,
            frames: self.frames,
            peak,
            rms,
            freq_hz: estimate_hz(&self.samples),
            wav: wav_bytes(&self.samples, SAMPLE_RATE),
        }
    }
}

/// Dominant frequency from rising zero crossings of the left channel.
fn estimate_hz(interleaved: &[i16]) -> f64 {
    let left: Vec<i16> = interleaved.chunks_exact(2).map(|f| f[0]).collect();
    let crossings: Vec<usize> = left
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0 && w[1] >= 0)
        .map(|(i, _)| i)
        .collect();
    if crossings.len() < 3 {
        return 0.0;
    }
    let span = (crossings[crossings.len() - 1] - crossings[0]) as f64;
    (crossings.len() - 1) as f64 * f64::from(SAMPLE_RATE) / span
}

/// The QEMU + native PL engine.
pub struct QemuEngine {
    def: BoardDef,
    board: BoardState,
    machine: QemuMachine,
    pl: TracedPl,
    /// Protocol-checker violations are errors (`[rtl] protocol_checkers = "strict"`).
    protocol_strict: bool,
    trace: TraceLog,
    uart1: ByteLog,
    /// Bytes the PL's UARTs (AXI UART Lite) transmitted, by channel; `uart_read` port 2 + n.
    pl_uart: Vec<ByteLog>,
    target_ns: u64,
    quantum_ns: u64,
    pl_name: String,
    uart_tcp: bool,
    media: Media,
    exited: bool,
    failure: Option<String>,
    work_dir: PathBuf,
    pl_finish_noted: bool,
    spec: QemuSpec,
    /// Keep the run directory (logs, XSim build, waveform) when the engine ends.
    keep_dir: bool,
    /// Every state-changing request applied so far, run targets normalised to
    /// `RunUntil`. Re-applying it to a fresh engine reproduces this one.
    journal: Vec<Command>,
    journal_bytes: usize,
    /// False once the journal outgrew its limits (snapshots/replay then say so).
    journal_ok: bool,
    /// Symbols of core 0's ELF (empty if it has none).
    symbols: Symbols,
    /// DWARF line table of core 0's ELF (empty if it has none).
    source: SourceMap,
    /// Where to look for the files the line table names.
    source_dirs: Vec<PathBuf>,
    /// Devices on Pmod ports; shared with the PL adapter, which feeds them
    /// pin changes the moment they arrive (ADR-009).
    devs: Arc<Mutex<devices::Bank>>,
    rgb: RgbMeter,
    snapshots: Vec<Snapshot>,
    /// Engines this one has been rebuilt from (names run directories).
    generation: u32,
    debug: Option<DebugState>,
    /// The board's Ethernet capture, when `[network]` is configured.
    net: Option<NetLog>,
    /// What is attached to the SD slot and QSPI socket, for the status.
    storage: Option<StorageStatus>,
    /// Guest writes reach host image files: re-execution cannot reproduce the
    /// session, so snapshot restore, reset and recording are refused.
    non_repeatable: bool,
}

/// Debugger bookkeeping of an engine started with `[debug]`.
struct DebugState {
    symbols: Symbols,
    breakpoints: Vec<BreakpointInfo>,
    /// Where the guest is stopped, if it is.
    stopped: Option<StopNote>,
    stops: u32,
}

struct StopNote {
    core: u8,
    pc: u32,
    reason: String,
}

fn cpsr_mode(cpsr: u32) -> &'static str {
    match cpsr & 0x1F {
        0x10 => "usr",
        0x11 => "fiq",
        0x12 => "irq",
        0x13 => "svc",
        0x16 => "mon",
        0x17 => "abt",
        0x1A => "hyp",
        0x1B => "und",
        0x1F => "sys",
        _ => "?",
    }
}

fn gdb_kind(k: BreakKind) -> GdbKind {
    match k {
        BreakKind::Exec => GdbKind::Exec,
        BreakKind::Write => GdbKind::Write,
        BreakKind::Read => GdbKind::Read,
        BreakKind::Access => GdbKind::Access,
    }
}

/// Splits `file.rs:42` into its parts (the line is after the last colon, so a
/// Windows drive letter is fine). `None` for anything else.
fn split_file_line(t: &str) -> Option<(&str, u32)> {
    let (file, line) = t.rsplit_once(':')?;
    let line: u32 = line.trim().parse().ok()?;
    let file = file.trim();
    (!file.is_empty() && line > 0).then_some((file, line))
}

/// The source position of an address, when the ELF has line information.
fn source_ref(map: &SourceMap, addr: u32) -> Option<SourceRef> {
    map.lookup(addr).map(|l| SourceRef {
        file: l.file.to_string(),
        line: l.line,
    })
}

/// An address (`0x100614`, `1049108`), an ELF symbol name or `file.rs:LINE`.
/// A source line may start code in several places; all are returned.
fn resolve_locations(
    text: &str,
    symbols: &Symbols,
    source: &SourceMap,
) -> Result<Vec<u32>, ApiError> {
    let t = text.trim();
    let num = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => t.parse::<u32>().ok(),
    };
    if let Some(a) = num.or_else(|| symbols.by_name(t).map(|s| s.addr)) {
        return Ok(vec![a]);
    }
    if let Some((file, line)) = split_file_line(t) {
        let found = source
            .addrs_for(file, line)
            .map_err(|m| ApiError::not_found(format!("breakpoint `{t}`: {m}")))?;
        return Ok(found.addrs);
    }
    Err(ApiError::not_found(format!(
        "`{t}` is neither an address, a symbol nor a `file.rs:LINE` of the ELF"
    )))
}

/// Averages the RGB LED pins into PWM duty cycles.
#[derive(Default)]
struct RgbMeter {
    /// Levels of the six pins now.
    levels: u8,
    /// PL time of the last accounting, in nanoseconds.
    last_ns: u64,
    /// High time per pin since the window started.
    high_ns: [u64; 6],
    /// Window start.
    since_ns: u64,
}

impl RgbMeter {
    /// Accounts the time up to `t_ns` at the current levels.
    fn advance(&mut self, t_ns: u64) {
        let dt = t_ns.saturating_sub(self.last_ns);
        for (i, h) in self.high_ns.iter_mut().enumerate() {
            if self.levels >> i & 1 != 0 {
                *h += dt;
            }
        }
        self.last_ns = self.last_ns.max(t_ns);
    }

    /// The duty of every pin if the window is long enough to mean something,
    /// starting a new window.
    fn take(&mut self, min_ns: u64) -> Option<[u8; 6]> {
        let span = self.last_ns.saturating_sub(self.since_ns);
        if span < min_ns {
            return None;
        }
        let duty =
            std::array::from_fn(|i| ((self.high_ns[i] * 255 + span / 2) / span).min(255) as u8);
        self.high_ns = [0; 6];
        self.since_ns = self.last_ns;
        Some(duty)
    }
}

/// A named point in the journal, with what a faithful restore must reproduce.
struct Snapshot {
    name: String,
    steps: usize,
    now_ns: u64,
    trace_len: usize,
    trace_hash: String,
}

impl QemuEngine {
    /// Starts QEMU with `spec` and places the native models.
    pub fn new(spec: &QemuSpec) -> Result<QemuEngine, ApiError> {
        let def = match &spec.board_def {
            Some(d) => d.clone(),
            None => BoardDef::builtin(&spec.board)
                .ok_or_else(|| ApiError::not_found(format!("unknown board `{}`", spec.board)))?,
        };
        let rtl_pl: Option<Box<dyn PlTarget + Send>> = match &spec.rtl {
            Some(rtl) => {
                let cfg = backend_rtl_xsim::XsimConfig {
                    vivado_root: rtl.vivado_root.clone(),
                    sources: rtl.sources.clone(),
                    top: rtl.top.clone(),
                    defines: rtl.defines.clone(),
                    dpi_lib: rtl.dpi_lib.clone(),
                    work_dir: spec.work_dir.join("xsim"),
                    waves: rtl.waves,
                    connect_timeout: std::time::Duration::from_secs(300),
                    cache_dir: std::env::var_os("ZYNQSIM_NO_BUILD_CACHE")
                        .is_none()
                        .then(|| spec.work_dir.parent().map(|p| p.join("xsim-cache")))
                        .flatten(),
                };
                Some(Box::new(
                    backend_rtl_xsim::XsimPl::start_with(&cfg, rtl.export.as_ref()).map_err(
                        |e| {
                            ApiError::io(e.to_string())
                                .with_hint("see build.log / xsim.out.log in the run directory")
                        },
                    )?,
                ))
            }
            None => None,
        };
        let pl: Box<dyn PlTarget + Send> = if spec.natives.is_empty() {
            rtl_pl.unwrap_or_else(|| Box::new(NativePl::new()))
        } else {
            // Native models (the prototype's math IP, the vendor-ip library), next to the
            // RTL design when there is one.
            let mut vp = VendorPl::new(rtl_pl);
            let mut vendor: Vec<VendorInstance> = Vec::new();
            for n in &spec.natives {
                if n.gp > 1 {
                    return Err(ApiError::invalid_argument(format!(
                        "instance `{}`: M_AXI_GP{} does not exist",
                        n.name, n.gp
                    )));
                }
                if vendor_ip::is_model(&n.model) {
                    let inst = VendorInstance {
                        name: n.name.clone(),
                        model: n.model.clone(),
                        gp: n.gp,
                        base: n.base,
                        size: n.size,
                        irq: n.irq,
                        irqs: n.irqs.clone(),
                        params: n.params.clone(),
                        bindings: n.bindings.clone(),
                    };
                    vp.add(&inst).map_err(ApiError::invalid_argument)?;
                    vendor.push(inst);
                } else {
                    let block = model(&n.model).ok_or_else(|| {
                        ApiError::invalid_argument(format!(
                            "unknown native model `{}` for instance `{}`; available: {}",
                            n.model,
                            n.name,
                            NATIVE_MODELS.join(", ")
                        ))
                    })?;
                    vp.add_block(&n.name, n.gp, n.base, block)
                        .map_err(ApiError::invalid_argument)?;
                }
            }
            vp.check_routes(&vendor)
                .map_err(ApiError::invalid_argument)?;
            Box::new(vp)
        };

        let mut cfg = QemuConfig::new(
            spec.qemu.clone(),
            spec.dtb.clone(),
            spec.elf0.clone(),
            spec.work_dir.clone(),
        );
        cfg.elf1 = spec.elf1.clone();
        cfg.quantum_ns = spec.quantum_ns;
        cfg.icount_shift = Some(spec.icount_shift);
        cfg.uart = spec.uart;
        cfg.debug = spec.debug;
        cfg.net = spec.net.clone();
        cfg.ps_extra_blocks = spec.ps_extra_blocks;
        let storage = Self::attach_storage(spec, &mut cfg)?;
        let mut machine = QemuMachine::spawn(&cfg).map_err(map_err)?;
        let mut pl = TracedPl {
            inner: pl,
            events: Vec::new(),
        };
        let mut bank = devices::Bank::new();
        for (port, kind) in &spec.devices {
            bank.attach(*port, make_device(*port, kind)?)
                .map_err(ApiError::invalid_argument)?;
        }
        for (port, name, value) in &spec.device_values {
            let dev = bank.get_mut(*port).ok_or_else(|| {
                ApiError::invalid_argument(format!("no device is attached to Pmod port {port}"))
            })?;
            devices::set_checked(dev, name, *value).map_err(ApiError::invalid_argument)?;
        }
        // Host-driven inputs (a Pmod BTN's buttons) reach the PL through the
        // input path, which the testbench samples: give it the starting levels.
        for (port, dev) in bank.iter() {
            if dev.drive().mask != 0 {
                pl.set_pin_inputs(port, bank.input_levels(port), 0);
            }
        }
        let devs = Arc::new(Mutex::new(bank));
        pl.set_pin_sink(Box::new(BankSink(Arc::clone(&devs))));
        let symbols = Symbols::load(&spec.elf0).unwrap_or_default();
        let source = SourceMap::load(&spec.elf0).unwrap_or_default();
        let debug = if spec.debug {
            machine.attach_debugger(&mut pl).map_err(map_err)?;
            let symbols = symbols.clone();
            let mut breakpoints = Vec::new();
            for b in &spec.breakpoints {
                for addr in resolve_locations(b, &symbols, &source)? {
                    machine
                        .debug_break(GdbKind::Exec, true, addr, 4)
                        .map_err(|e| ApiError::io(e.to_string()))?;
                    breakpoints.push(BreakpointInfo {
                        addr: u64::from(addr),
                        kind: BreakKind::Exec,
                        len: 4,
                        symbol: symbols.containing(addr).map(|(s, _)| s.name.clone()),
                        source: source_ref(&source, addr),
                    });
                }
            }
            Some(DebugState {
                symbols,
                breakpoints,
                stopped: None,
                stops: 0,
            })
        } else {
            None
        };

        let mut trace = TraceLog::with_limit(TRACE_RETAIN);
        let elf = spec
            .elf0
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        trace.push(
            Ps::ZERO,
            TraceEvent::Marker {
                text: format!(
                    "qemu start elf={elf} quantum_ns={} icount_shift={}",
                    spec.quantum_ns, spec.icount_shift
                ),
            },
        );
        // Not in the trace: the path differs per run and would change the digest.
        if std::env::var_os("ZYNQSIM_KEEP_RUN").is_some()
            || spec.rtl.as_ref().is_some_and(|r| r.waves)
        {
            eprintln!("zynqsim: run directory kept: {}", spec.work_dir.display());
        }
        if let Some(s) = &storage {
            // The part of the configuration that changes what the guest sees
            // belongs in the digest. Names only: host paths differ per machine.
            let part = |d: &Option<StorageDevice>| match d {
                Some(d) => format!("{}:{}:{}B", d.model, d.source, d.bytes),
                None => "none".to_string(),
            };
            trace.push(
                Ps::ZERO,
                TraceEvent::Marker {
                    text: format!(
                        "storage sd={} qspi={} writable={}",
                        part(&s.sd),
                        part(&s.qspi),
                        spec.storage.writable
                    ),
                },
            );
        }
        Ok(QemuEngine {
            board: BoardState::new(&def),
            def,
            machine,
            pl,
            trace,
            uart1: ByteLog::default(),
            pl_uart: Vec::new(),
            target_ns: 0,
            quantum_ns: spec.quantum_ns,
            pl_name: spec.pl_name.clone(),
            uart_tcp: spec.uart == UartMode::Tcp,
            media: Media::default(),
            exited: false,
            failure: None,
            work_dir: spec.work_dir.clone(),
            pl_finish_noted: false,
            protocol_strict: spec.rtl.as_ref().is_some_and(|r| r.protocol_strict),
            spec: spec.clone(),
            keep_dir: std::env::var_os("ZYNQSIM_KEEP_RUN").is_some()
                || spec.rtl.as_ref().is_some_and(|r| r.waves),
            journal: Vec::new(),
            journal_bytes: 0,
            journal_ok: true,
            symbols,
            source,
            source_dirs: spec.source_dirs.clone(),
            devs,
            rgb: RgbMeter::default(),
            snapshots: Vec::new(),
            generation: 0,
            debug,
            net: spec.net.as_ref().map(NetLog::new),
            storage,
            non_repeatable: spec.storage.mutates_host_files(),
        })
    }

    /// Resolves the storage spec into QEMU arguments and the status report.
    /// A folder card is written into the run directory here.
    fn attach_storage(
        spec: &QemuSpec,
        cfg: &mut QemuConfig,
    ) -> Result<Option<StorageStatus>, ApiError> {
        let st = &spec.storage;
        if !st.any() {
            return Ok(None);
        }
        let mut status = StorageStatus::default();
        cfg.storage.writable = st.writable;
        // Resolves one source to the file QEMU opens, its size and the name
        // shown in the status. A generated image goes into the run directory.
        let resolve = |src: &ImageSource,
                       what: &str,
                       file: &str|
         -> Result<(PathBuf, u64, String, bool), ApiError> {
            match src {
                ImageSource::File(p) => {
                    let bytes = std::fs::metadata(p)
                        .map_err(|e| {
                            ApiError::not_found(format!("{what} image {}: {e}", p.display()))
                        })?
                        .len();
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    Ok((p.clone(), bytes, name, st.writable))
                }
                ImageSource::Generated { name, image } => {
                    std::fs::create_dir_all(&spec.work_dir).map_err(|e| {
                        ApiError::io(format!("cannot create the run directory: {e}"))
                    })?;
                    let p = spec.work_dir.join(file);
                    std::fs::write(&p, image.as_slice())
                        .map_err(|e| ApiError::io(format!("cannot write the {what} image: {e}")))?;
                    // Rebuilt for every run: writes live for the run only and
                    // never reach the host file or folder.
                    Ok((p, image.len() as u64, name.clone(), false))
                }
            }
        };
        if let Some(src) = &st.sd {
            let (path, bytes, source, writable) = resolve(src, "SD", "sdcard.img")?;
            cfg.storage.sd_image = Some(path);
            status.sd = Some(StorageDevice {
                source,
                bytes,
                model: sd_model(bytes),
                writable,
            });
        }
        if let Some(src) = &st.qspi {
            let (path, bytes, source, writable) = resolve(src, "QSPI", "qspi.bin")?;
            let model = st
                .qspi_model
                .clone()
                .unwrap_or_else(|| crate::storage::DEFAULT_QSPI_MODEL.to_string());
            cfg.storage.qspi_image = Some(path);
            cfg.storage.qspi_model = Some(model.clone());
            status.qspi = Some(StorageDevice {
                source,
                bytes,
                model,
                writable,
            });
        }
        Ok(Some(status))
    }

    /// Builds a fresh engine from the same spec and applies `steps` to it.
    fn rebuild(&self, steps: &[Command]) -> Result<QemuEngine, ApiError> {
        let mut spec = self.spec.clone();
        let base = spec
            .work_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "run".to_string());
        // The original directory name without an earlier "-rN" suffix.
        let base = base.split("-r").next().unwrap_or(&base).to_string();
        spec.work_dir = spec
            .work_dir
            .with_file_name(format!("{base}-r{}", self.generation + 1));
        let mut fresh = QemuEngine::new(&spec)?;
        fresh.generation = self.generation + 1;
        for c in steps {
            fresh.command(c.clone())?;
        }
        Ok(fresh)
    }

    fn journal_needed(&self, what: &str) -> Result<(), ApiError> {
        if self.non_repeatable {
            return Err(self.repeatability_error(what));
        }
        if self.journal_ok {
            Ok(())
        } else {
            Err(self.unsupported(
                what,
                "the session grew past the journal limits (2 million requests or 64 MiB of \
                 inputs); reset to start recording again",
            ))
        }
    }

    fn record(&mut self, c: Command) {
        if !self.journal_ok {
            return;
        }
        let bytes = match &c {
            Command::MemWrite { data, .. } => data.len(),
            Command::SetMediaSource { video, audio } => {
                let v = match video {
                    Some(VideoSpec::Image { rgb, .. }) => rgb.len(),
                    _ => 0,
                };
                let a = match audio {
                    Some(AudioSpec::Pcm { samples }) => samples.len() * 2,
                    _ => 0,
                };
                v + a
            }
            _ => 0,
        } + 48;
        self.journal_bytes += bytes;
        if self.journal.len() >= JOURNAL_MAX_STEPS || self.journal_bytes > JOURNAL_MAX_BYTES {
            self.journal_ok = false;
            self.journal = Vec::new();
            self.snapshots.clear();
            return;
        }
        self.journal.push(c);
    }

    fn snapshot_save(&mut self, name: String) -> Result<Response, ApiError> {
        self.journal_needed("snapshot")?;
        self.snapshots.retain(|s| s.name != name);
        self.snapshots.push(Snapshot {
            name,
            steps: self.journal.len(),
            now_ns: self.machine.now_ns(),
            trace_len: self.trace.len(),
            trace_hash: self.trace.hash_string(),
        });
        Ok(Response::Ok)
    }

    /// Restores by deterministic re-execution: a new machine runs the journal
    /// up to the snapshot and the result must match the digest taken then.
    fn snapshot_restore(&mut self, name: &str) -> Result<Response, ApiError> {
        self.journal_needed("snapshot")?;
        let (steps, now_ns, trace_len, hash) = {
            let s = self
                .snapshots
                .iter()
                .find(|s| s.name == name)
                .ok_or_else(|| ApiError::not_found(format!("no snapshot named `{name}`")))?;
            (s.steps, s.now_ns, s.trace_len, s.trace_hash.clone())
        };
        let mut fresh = self.rebuild(&self.journal[..steps])?;
        if fresh.trace.hash_string() != hash
            || fresh.trace.len() != trace_len
            || fresh.machine.now_ns() != now_ns
        {
            return Err(ApiError::internal(format!(
                "re-execution did not reproduce snapshot `{name}` (digest {} vs {hash}): the \
                 session is not deterministic, which happens with interactive (TCP) UART input",
                fresh.trace.hash_string()
            )));
        }
        fresh.snapshots = std::mem::take(&mut self.snapshots)
            .into_iter()
            .filter(|s| s.steps <= steps)
            .collect();
        std::mem::swap(self, &mut fresh);
        // `fresh` now holds the old machine; dropping it stops QEMU and XSim.
        Ok(Response::Ok)
    }

    fn repeatability_error(&self, what: &str) -> ApiError {
        self.unsupported(
            what,
            "this session writes to host image files (`[storage] writable = true`), so \
             re-executing it would start from a different disk than the original run did; \
             use the default read-only storage for snapshots, reset and recording",
        )
    }

    /// Starts over from time zero with the same firmware and PL design.
    fn reset_in_place(&mut self) -> Result<Response, ApiError> {
        if self.non_repeatable {
            return Err(self.repeatability_error("reset"));
        }
        let mut fresh = self.rebuild(&[])?;
        std::mem::swap(self, &mut fresh);
        // `fresh` holds the old machine and is dropped here.
        Ok(Response::Ok)
    }

    fn now_ps(&self) -> Ps {
        Ps(self.machine.now_ns().saturating_mul(1000))
    }

    /// The devices on the Pmod ports.
    fn bank(&self) -> std::sync::MutexGuard<'_, devices::Bank> {
        self.devs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Moves PL accesses and UART output into the trace and the UART log.
    fn flush(&mut self) {
        for e in self.pl.events.drain(..) {
            self.trace.push(
                Ps(e.t_ns.saturating_mul(1000)),
                TraceEvent::Bus {
                    port: format!("M_AXI_GP{}", e.port),
                    write: e.write,
                    addr: e.addr,
                    len: e.len,
                    data: e.data,
                    ok: e.ok,
                },
            );
        }
        let mut rgb_event = false;
        // AXI protocol-checker findings of the testbench (ADR-025), as trace markers.
        for f in self.pl.drain_protocol_findings() {
            self.trace.push(
                Ps(f.t_ns.saturating_mul(1000)),
                TraceEvent::Marker {
                    text: f.marker(self.protocol_strict),
                },
            );
        }
        for ev in self.pl.drain_pin_events() {
            let t = Ps(ev.t_ns.saturating_mul(1000));
            if ev.group == pl_api::PIN_GROUP_RGB {
                rgb_event = true;
                self.rgb.advance(ev.t_ns);
                self.rgb.levels = ev.levels & 0x3F;
                self.trace.push(
                    t,
                    TraceEvent::Signal {
                        name: "board.rgb_pins".to_string(),
                        value: u64::from(ev.levels & 0x3F),
                        width: 6,
                    },
                );
            } else if let Some(row) = self.board.pmod_pins.get_mut(usize::from(ev.group)) {
                for (i, p) in row.iter_mut().enumerate() {
                    *p = if ev.levels >> i & 1 != 0 {
                        board_zybo::PinLevel::High
                    } else {
                        board_zybo::PinLevel::Low
                    };
                }
                let name = format!(
                    "pmod.{}",
                    self.def
                        .pmods
                        .get(usize::from(ev.group))
                        .map_or_else(|| ev.group.to_string(), |p| p.id.to_lowercase())
                );
                self.trace.push(
                    t,
                    TraceEvent::Signal {
                        name,
                        value: u64::from(ev.levels),
                        width: 8,
                    },
                );
                // The devices were fed when the event arrived (`BankSink`).
            } else if ev.group == devices::CAMERA_PORT {
                self.trace.push(
                    t,
                    TraceEvent::Signal {
                        name: "cam.pins".to_string(),
                        value: u64::from(ev.levels),
                        width: 8,
                    },
                );
            }
        }
        // PWM duty of the RGB LEDs, from the pin activity so far. Once the
        // testbench has ended the pins are frozen at an arbitrary phase of
        // the PWM, which says nothing about the duty: keep the last result.
        if self.pl.has_pin_groups() && !self.pl.finished() {
            // Without new events the pins held their level up to now; with
            // events, the PL may lag the PS, so account only up to the last one.
            if !rgb_event {
                self.rgb.advance(self.machine.now_ns());
            }
            if let Some(duty) = self.rgb.take(200_000) {
                for (c, d) in self.board.rgb.iter_mut().zip(duty.chunks_exact(3)) {
                    c.copy_from_slice(d);
                }
            }
        }
        for o in self.pl.drain_board_outputs() {
            for i in 0..self.board.leds.len().min(board_zybo::MAX_BITS) {
                let duty = if o.leds & (1 << i) != 0 { 255 } else { 0 };
                if self.board.leds[i] != duty {
                    self.board.leds[i] = duty;
                    self.trace.push(
                        Ps(o.t_ns.saturating_mul(1000)),
                        TraceEvent::Signal {
                            name: format!("board.{}", self.def.leds[i].id),
                            value: u64::from(duty),
                            width: 8,
                        },
                    );
                }
            }
        }
        for ev in self.pl.drain_media() {
            match ev {
                MediaEvent::Audio {
                    first_frame,
                    samples,
                    ..
                } => self.media.push_audio(first_frame, &samples),
                MediaEvent::VideoLine { frame, y, rgb, .. } => {
                    self.media.pending.entry(frame).or_default().insert(y, rgb);
                    if self.media.pending.len() > PENDING_FRAMES {
                        if let Some(oldest) = self.media.pending.keys().min().copied() {
                            self.media.pending.remove(&oldest);
                        }
                    }
                }
                MediaEvent::VideoFrameEnd { frame, t_ns } => {
                    self.media.end_frame(frame);
                    self.trace.push(
                        Ps(t_ns.saturating_mul(1000)),
                        TraceEvent::Signal {
                            name: "hdmi.tx.frame".to_string(),
                            value: u64::from(frame),
                            width: 32,
                        },
                    );
                }
                MediaEvent::CodecRegister { t_ns, reg, value } => {
                    self.media.codec_writes += 1;
                    self.trace.push(
                        Ps(t_ns.saturating_mul(1000)),
                        TraceEvent::Signal {
                            name: format!("codec.r{reg}"),
                            value: u64::from(value),
                            width: 9,
                        },
                    );
                }
            }
        }
        for m in self.pl.drain_master_log() {
            self.trace.push(
                Ps(m.t_ns.saturating_mul(1000)),
                TraceEvent::Bus {
                    port: master_port_name(m.port).to_string(),
                    write: m.write,
                    addr: m.addr,
                    len: m.len,
                    data: m.data,
                    ok: m.ok,
                },
            );
        }
        if self.pl.finished() && !self.pl_finish_noted {
            self.pl_finish_noted = true;
            let t = self.now_ps();
            self.trace.push(
                t,
                TraceEvent::Marker {
                    text: "pl finished".to_string(),
                },
            );
        }
        if let Some(net) = &mut self.net {
            let raw = self.machine.net_take();
            for (t_ns, ev) in net.ingest(raw, self.machine.now_ns()) {
                self.trace.push(Ps(t_ns.saturating_mul(1000)), ev);
            }
        }
        let now = self.now_ps();
        for b in self.machine.uart_take() {
            self.trace.push(
                now,
                TraceEvent::Uart {
                    port: 1,
                    tx: true,
                    byte: b,
                },
            );
            self.uart1.push(b);
        }
        for b in self.pl.drain_uart_tx() {
            let ch = usize::from(b.channel);
            if self.pl_uart.len() <= ch {
                self.pl_uart.resize_with(ch + 1, ByteLog::default);
            }
            self.pl_uart[ch].push(b.byte);
            self.trace.push(
                Ps(b.t_ns.saturating_mul(1000)),
                TraceEvent::Uart {
                    port: 2 + b.channel,
                    tx: true,
                    byte: b.byte,
                },
            );
        }
    }

    /// Runs QEMU to `end_ns`, in chunks that end on multiples of [`CHUNK_NS`],
    /// flushing after each.
    ///
    /// UART bytes carry no timestamp of their own (the chardev is a file), so
    /// each is attributed to the virtual time at which it was observed. The
    /// trace is therefore identical for identical sequences of run requests,
    /// not for arbitrary re-slicing of the same total time.
    fn advance_to_ns(&mut self, end_ns: u64) -> Result<(), ApiError> {
        if let Some(f) = &self.failure {
            return Err(ApiError::io(f.clone()));
        }
        self.target_ns = self.target_ns.max(end_ns);
        if self.debug.is_some() {
            // Running again (also the very first start): leaves any stop.
            self.machine
                .resume()
                .map_err(|e| ApiError::io(e.to_string()))?;
            if let Some(d) = &mut self.debug {
                d.stopped = None;
            }
        }
        while !self.exited && self.machine.now_ns() < self.target_ns {
            let boundary = (self.machine.now_ns() / CHUNK_NS + 1) * CHUNK_NS;
            let t = boundary.min(self.target_ns);
            let end = self.machine.run_until(t, &mut self.pl, WALL_LIMIT);
            self.flush();
            match end {
                Ok(RunEnd::ReachedTime) => {}
                Ok(RunEnd::Stopped(stop)) => {
                    self.note_stop(stop);
                    break;
                }
                Ok(RunEnd::PeerClosed) => {
                    self.exited = true;
                    let now = self.now_ps();
                    self.trace.push(
                        now,
                        TraceEvent::Marker {
                            text: "qemu exited".to_string(),
                        },
                    );
                }
                Err(e) => {
                    self.failure = Some(e.to_string());
                    self.exited = true;
                    return Err(map_err(e));
                }
            }
        }
        self.settle_uart();
        Ok(())
    }

    /// Records where the guest stopped and marks the trace.
    fn note_stop(&mut self, stop: backend_qemu::Stop) {
        let core = stop.thread.saturating_sub(1);
        let regs = self.machine.debug_regs(core).ok();
        let pc = regs.as_ref().map_or(0, |r| r.r[15]);
        let Some(d) = &mut self.debug else { return };
        let sym = d
            .symbols
            .containing(pc)
            .map(|(s, off)| {
                if off == 0 {
                    s.name.clone()
                } else {
                    format!("{}+{off:#x}", s.name)
                }
            })
            .unwrap_or_default();
        let reason = match stop.watch_addr {
            Some(a) => format!("watchpoint at {a:#x}"),
            None if d.breakpoints.iter().any(|b| b.addr == u64::from(pc)) => {
                "breakpoint".to_string()
            }
            None => "stop".to_string(),
        };
        let at = source_ref(&self.source, pc)
            .map(|s| format!("{}:{}", s.file, s.line))
            .unwrap_or_default();
        let text = format!(
            "core {core} stopped: {reason} at pc={pc:#010x}{}",
            match (sym.is_empty(), at.is_empty()) {
                (true, true) => String::new(),
                (false, true) => format!(" ({sym})"),
                (true, false) => format!(" ({at})"),
                (false, false) => format!(" ({sym}, {at})"),
            }
        );
        d.stops += 1;
        d.stopped = Some(StopNote {
            core,
            pc,
            reason: text.clone(),
        });
        let now = self.now_ps();
        self.trace.push(now, TraceEvent::Marker { text });
    }

    fn debug_needed(&self, what: &str) -> Result<&DebugState, ApiError> {
        self.debug.as_ref().ok_or_else(|| {
            self.unsupported(
                what,
                "the debugger is not attached: set `[debug] enabled = true` in project.toml \
                 (timing is then not reproducible between runs)",
            )
        })
    }

    /// In TCP UART mode the host reads the socket asynchronously, so bytes the
    /// guest has already sent may not have arrived when a run ends. Wait until
    /// the stream has been quiet for a moment so a report taken right after a
    /// run is complete. (The file mode needs no such wait: it is exact.)
    fn settle_uart(&mut self) {
        if !self.uart_tcp {
            return;
        }
        let mut quiet = 0;
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(8));
            let before = self.uart1.end();
            self.flush();
            if self.uart1.end() == before {
                quiet += 1;
                if quiet >= 2 {
                    break;
                }
            } else {
                quiet = 0;
            }
        }
    }

    fn require_pins(&self) -> Result<(), ApiError> {
        if self.pl.has_board_pins() {
            Ok(())
        } else {
            Err(self.unsupported(
                "board inputs",
                "this PL is not connected to the board pins (only an RTL design in XSim is)",
            ))
        }
    }

    fn push_board_inputs(&mut self) {
        let bits = |v: &[bool]| {
            // Bit n = component n of the board; a board file has at most 8 (ADR-022).
            v.iter()
                .take(board_zybo::MAX_BITS)
                .enumerate()
                .fold(0u8, |a, (i, &b)| a | (u8::from(b) << i))
        };
        let (sw, btn) = (bits(&self.board.switches), bits(&self.board.buttons));
        self.pl.set_board_inputs(sw, btn, self.machine.now_ns());
    }

    fn unsupported(&self, what: &str, why: &str) -> ApiError {
        ApiError::unsupported(format!(
            "`{what}` is not available on the `qemu` backend yet: {why}"
        ))
    }

    fn net_log(&self, what: &str) -> Result<&NetLog, ApiError> {
        self.net.as_ref().ok_or_else(|| {
            self.unsupported(
                what,
                "the board's Ethernet is not attached; add a `[network]` section to project.toml",
            )
        })
    }
}

impl Drop for QemuEngine {
    fn drop(&mut self) {
        // Stop QEMU so it releases its log files, then remove the run
        // directory. After a failure the logs are the evidence: keep them.
        self.machine.kill();
        if self.failure.is_none() && !self.keep_dir {
            let _ = std::fs::remove_dir_all(&self.work_dir);
            if let Some(parent) = self.work_dir.parent() {
                // The shared scratch root is removed only when nothing else is in it.
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

impl Engine for QemuEngine {
    fn def(&self) -> &BoardDef {
        &self.def
    }

    fn board(&self) -> &BoardState {
        &self.board
    }

    fn trace(&self) -> &TraceLog {
        &self.trace
    }

    fn status(&self, playing: bool) -> Status {
        // Uart is advertised for reading; writing needs the TCP mode and is
        // rejected by the command with an explanation otherwise.
        let mut caps = vec![
            Capability::Uart,
            Capability::Trace,
            Capability::Screenshot,
            Capability::MemoryAccess,
        ];
        if self.debug.is_some() {
            caps.push(Capability::CpuState);
        }
        if !self.non_repeatable {
            caps.push(Capability::Snapshot);
            caps.push(Capability::Replay);
        }
        if self.storage.is_some() {
            caps.push(Capability::Storage);
        }
        if self.pl.has_board_pins() {
            caps.push(Capability::BoardIo);
        }
        if self.pl.has_pin_groups() {
            caps.push(Capability::Pmod);
        }
        if self.pl.has_media() {
            caps.push(Capability::Video);
            caps.push(Capability::Audio);
        }
        if self.net.is_some() {
            caps.push(Capability::Network);
        }
        Status {
            now_ps: self.now_ps().0,
            playing,
            quantum_ps: self.quantum_ns * 1000,
            seed: 0,
            board: self.def.name.clone(),
            ps_backend: "qemu".to_string(),
            pl_backend: self.pl_name.clone(),
            fidelity: Fidelity::F0,
            capabilities: caps,
            trace_len: self.trace.len(),
            trace_hash: self.trace.hash_string(),
            pending_events: 0,
            devices: self
                .bank()
                .iter()
                .map(|(port, d)| DeviceStatus {
                    port,
                    kind: d.kind().to_string(),
                    description: d.describe(),
                })
                .collect(),
            debug: self.debug.as_ref().map(|d| DebugStatus {
                stopped: d.stopped.is_some(),
                core: d.stopped.as_ref().map(|s| s.core),
                pc: d.stopped.as_ref().map(|s| s.pc),
                reason: d.stopped.as_ref().map(|s| s.reason.clone()),
                stops: d.stops,
            }),
            media: self.pl.has_media().then(|| self.media.status()),
            storage: self.storage.clone(),
        }
    }

    fn command(&mut self, cmd: Command) -> Result<Response, ApiError> {
        match cmd {
            Command::SnapshotSave { name } => return self.snapshot_save(name),
            Command::SnapshotRestore { name } => return self.snapshot_restore(&name),
            Command::Input(Input::Reset { .. }) => return self.reset_in_place(),
            _ => {}
        }
        // Run targets are journaled as absolute `RunUntil`s so a replay slices
        // the run exactly as it was sliced here.
        let cmd = match cmd {
            Command::Run { duration_ps } => Command::RunUntil {
                time_ps: (self.target_ns.max(self.machine.now_ns()) + duration_ps / 1000) * 1000,
            },
            Command::Step => Command::RunUntil {
                time_ps: (self.target_ns.max(self.machine.now_ns()) + self.quantum_ns) * 1000,
            },
            Command::RunUntil { time_ps } => Command::RunUntil {
                time_ps: time_ps / 1000 * 1000,
            },
            other => other,
        };
        let journaled = !matches!(cmd, Command::Play { .. } | Command::Pause);
        let r = self.apply(cmd.clone());
        if r.is_ok() && journaled {
            self.record(cmd);
        }
        r
    }

    fn query(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        self.query_inner(q, playing)
    }

    fn run_for(&mut self, d: Ps) -> Result<(), ApiError> {
        self.command(Command::Run { duration_ps: d.0 }).map(|_| ())
    }

    fn halted(&self) -> Option<String> {
        self.debug
            .as_ref()
            .and_then(|d| d.stopped.as_ref())
            .map(|s| s.reason.clone())
    }
}

impl QemuEngine {
    fn apply(&mut self, cmd: Command) -> Result<Response, ApiError> {
        match cmd {
            Command::Run { duration_ps } => {
                let end = self.target_ns.max(self.machine.now_ns()) + duration_ps / 1000;
                self.advance_to_ns(end)?;
                Ok(Response::Time {
                    now_ps: self.now_ps().0,
                })
            }
            Command::RunUntil { time_ps } => {
                self.advance_to_ns(time_ps / 1000)?;
                Ok(Response::Time {
                    now_ps: self.now_ps().0,
                })
            }
            Command::Step => {
                let end = self.target_ns.max(self.machine.now_ns()) + self.quantum_ns;
                self.advance_to_ns(end)?;
                Ok(Response::Time {
                    now_ps: self.now_ps().0,
                })
            }
            Command::Input(Input::UartWrite { port, data }) if port >= 2 => {
                // A PL UART (AXI UART Lite): deterministic, delivered at the current time.
                let ch = port - 2;
                if usize::from(ch) >= self.pl.uart_channels().len() {
                    return Err(ApiError::invalid_argument(format!(
                        "no uart {port}: the PL has {} UART Lite instance(s) (ports 2..)",
                        self.pl.uart_channels().len()
                    )));
                }
                let now_ns = self.machine.now_ns();
                let n = self.pl.uart_rx(ch, &data, now_ns);
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("uart{port} write {n} byte(s)"),
                    },
                );
                if n < data.len() {
                    return Err(ApiError::invalid_argument(format!(
                        "uart{port}: only {n} of {} byte(s) fit the receive queue",
                        data.len()
                    )));
                }
                Ok(Response::Ok)
            }
            Command::Input(Input::UartWrite { port, data }) => {
                if port != 1 {
                    return Err(self.unsupported(
                        "uart_write",
                        "only UART1 is connected (UART0 is a null device)",
                    ));
                }
                self.machine.uart_write(&data).map_err(|e| {
                    ApiError::unsupported(format!(
                        "UART input is unavailable: {e}. Select the interactive UART mode \
                         (`[host] uart1 = \"tcp\"`); its delivery time is not deterministic."
                    ))
                })?;
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("uart1 write {} byte(s)", data.len()),
                    },
                );
                Ok(Response::Ok)
            }
            Command::Input(Input::SetSwitch { index, on }) => {
                self.require_pins()?;
                if index >= self.board.switches.len() {
                    return Err(ApiError::invalid_argument(format!(
                        "switch index {index} out of range 0..{}",
                        self.board.switches.len()
                    )));
                }
                self.board.switches[index] = on;
                let id = self.def.switches[index].id.clone();
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("switch {index} = {}", if on { "on" } else { "off" }),
                    },
                );
                self.trace.push(
                    now,
                    TraceEvent::Signal {
                        name: format!("board.{id}"),
                        value: u64::from(on),
                        width: 1,
                    },
                );
                self.push_board_inputs();
                Ok(Response::Ok)
            }
            Command::Input(Input::SetButton { index, pressed }) => {
                self.require_pins()?;
                if index >= self.board.buttons.len() {
                    return Err(ApiError::invalid_argument(format!(
                        "button index {index} out of range 0..{}",
                        self.board.buttons.len()
                    )));
                }
                self.board.buttons[index] = pressed;
                let id = self.def.buttons[index].id.clone();
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!(
                            "button {index} = {}",
                            if pressed { "pressed" } else { "released" }
                        ),
                    },
                );
                self.trace.push(
                    now,
                    TraceEvent::Signal {
                        name: format!("board.{id}"),
                        value: u64::from(pressed),
                        width: 1,
                    },
                );
                self.push_board_inputs();
                Ok(Response::Ok)
            }
            Command::AttachDevice { port, kind } => {
                if !self.pl.has_pin_groups() {
                    return Err(self.unsupported(
                        "pmod devices",
                        "this PL does not report Pmod pins (an RTL design using the zs_pin_* DPI)",
                    ));
                }
                let dev = make_device(port, &kind)?;
                self.bank()
                    .attach(port, dev)
                    .map_err(ApiError::invalid_argument)?;
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("attach {kind} to Pmod port {port}"),
                    },
                );
                Ok(Response::Ok)
            }
            Command::SetDeviceValue { port, name, value } => {
                if !self.pl.has_pin_groups() {
                    return Err(self.unsupported(
                        "pmod devices",
                        "this PL does not report Pmod pins (an RTL design using the zs_pin_* DPI)",
                    ));
                }
                let (stored, levels) = {
                    let mut bank = self.bank();
                    let dev = bank.get_mut(port).ok_or_else(|| {
                        ApiError::not_found(format!("no device is attached to Pmod port {port}"))
                    })?;
                    let stored = devices::set_checked(dev, &name, value)
                        .map_err(ApiError::invalid_argument)?;
                    (stored, bank.input_levels(port))
                };
                let now = self.now_ps();
                // The PL sees a host-driven input at its next sample (zs_pin_in).
                self.pl.set_pin_inputs(port, levels, now.0 / 1000);
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("set Pmod port {port} {name} = {stored}"),
                    },
                );
                Ok(Response::Ok)
            }
            Command::SetBreakpoint {
                addr,
                kind,
                len,
                enable,
            } => {
                self.debug_needed("breakpoint_set")?;
                let a = u32::try_from(addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                if kind != BreakKind::Exec && !(1..=64).contains(&len) {
                    return Err(ApiError::invalid_argument(
                        "a watchpoint covers 1 to 64 bytes",
                    ));
                }
                self.machine
                    .debug_break(gdb_kind(kind), enable, a, len)
                    .map_err(|e| ApiError::io(e.to_string()))?;
                if let Some(d) = &mut self.debug {
                    d.breakpoints
                        .retain(|b| !(b.addr == addr && b.kind == kind));
                    if enable {
                        let symbol = d.symbols.containing(a).map(|(s, _)| s.name.clone());
                        d.breakpoints.push(BreakpointInfo {
                            addr,
                            kind,
                            len: if kind == BreakKind::Exec { 4 } else { len },
                            symbol,
                            source: if kind == BreakKind::Exec {
                                source_ref(&self.source, a)
                            } else {
                                None
                            },
                        });
                    }
                }
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!(
                            "{} {kind:?} breakpoint at {addr:#x}",
                            if enable { "set" } else { "clear" }
                        ),
                    },
                );
                Ok(Response::Ok)
            }
            Command::SetMediaSource { video, audio } => {
                if !self.pl.has_media() {
                    return Err(self.unsupported(
                        "media sources",
                        "this PL has no HDMI/codec models (use an RTL design with zs_media.sv)",
                    ));
                }
                let v = match video {
                    None => None,
                    Some(VideoSpec::ColorBars { width, height }) => {
                        check_size(width, height)?;
                        Some(VideoSource::color_bars(width, height))
                    }
                    Some(VideoSpec::Gradient { width, height }) => {
                        check_size(width, height)?;
                        Some(VideoSource::gradient(width, height))
                    }
                    Some(VideoSpec::Image { width, height, rgb }) => {
                        check_size(width, height)?;
                        if rgb.len() != (width * height * 3) as usize {
                            return Err(ApiError::invalid_argument(format!(
                                "image data is {} bytes, expected {}",
                                rgb.len(),
                                width * height * 3
                            )));
                        }
                        Some(VideoSource { width, height, rgb })
                    }
                };
                let a = audio.map(|a| match a {
                    AudioSpec::Silence => AudioSource::Silence,
                    AudioSpec::Tone { freq_hz, amplitude } => {
                        AudioSource::Tone { freq_hz, amplitude }
                    }
                    AudioSpec::Pcm { samples } => AudioSource::Pcm(samples),
                });
                self.pl.set_media_sources(v, a);
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: "media source changed".to_string(),
                    },
                );
                Ok(Response::Ok)
            }
            Command::MemWrite { addr, data } => {
                if data.is_empty() || data.len() > 1 << 20 {
                    return Err(ApiError::invalid_argument(
                        "data must be between 1 byte and 1 MiB",
                    ));
                }
                self.machine
                    .write_memory(addr, &data)
                    .map_err(|e| ApiError::io(e.to_string()))?;
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("mem write {:#x} len {}", addr, data.len()),
                    },
                );
                Ok(Response::Ok)
            }
            Command::Input(Input::Reset { .. }) => self.reset_in_place(),
            Command::SetQuantum { .. } => Err(self.unsupported(
                "set_quantum",
                "the quantum is fixed when QEMU starts (`[sim] quantum_ns`)",
            )),
            Command::SnapshotSave { name } => self.snapshot_save(name),
            Command::SnapshotRestore { name } => self.snapshot_restore(&name),
            Command::Play { .. } | Command::Pause => Err(ApiError::unsupported(
                "continuous running needs the simulation server (SimClient)",
            )),
        }
    }

    fn query_inner(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        match q {
            Query::Status => Ok(Response::Status(Box::new(self.status(playing)))),
            Query::BoardDef => Ok(Response::BoardDef(Box::new(self.def.clone()))),
            Query::BoardState => Ok(Response::BoardState(self.board.clone())),
            Query::Pin { name } => self
                .board
                .pin_level(&self.def, name)
                .map(|level| Response::Pin {
                    name: name.clone(),
                    level,
                })
                .ok_or_else(|| ApiError::not_found(format!("no pin or control named `{name}`"))),
            Query::UartRead { port, from, max } => match port {
                1 => {
                    let (data, next) = self.uart1.read(*from, *max);
                    Ok(Response::Uart {
                        data,
                        next,
                        oldest: self.uart1.base,
                    })
                }
                0 => Ok(Response::Uart {
                    data: Vec::new(),
                    next: 0,
                    oldest: 0,
                }),
                // The PL's AXI UART Lite instances, in instance-name order.
                p if usize::from(*p) < 2 + self.pl.uart_channels().len() => {
                    let empty = ByteLog::default();
                    let log = self.pl_uart.get(usize::from(*p) - 2).unwrap_or(&empty);
                    let (data, next) = log.read(*from, *max);
                    Ok(Response::Uart {
                        data,
                        next,
                        oldest: log.base,
                    })
                }
                p => Err(ApiError::invalid_argument(format!("no uart {p}"))),
            },
            Query::Trace { from, max } => {
                let (start, recs) = self.trace.window(*from, *max);
                Ok(Response::Trace {
                    records: recs.to_vec(),
                    next: start + recs.len(),
                    total: self.trace.len(),
                    hash: self.trace.hash_string(),
                })
            }
            Query::TraceExport { format } => Ok(Response::Text(match format {
                TraceFormat::Vcd => trace::to_vcd(&self.trace),
                TraceFormat::Perfetto => trace::to_perfetto_json(&self.trace),
            })),
            Query::SignalCatalogue | Query::Waveform { .. } => {
                crate::wave::answer(&self.trace, q)
            }
            Query::Snapshots => Ok(Response::Snapshots(
                self.snapshots.iter().map(|s| s.name.clone()).collect(),
            )),
            Query::Recording => {
                self.journal_needed("recording")?;
                Ok(Response::Recording(Box::new(crate::Recording {
                    version: 1,
                    config: crate::SimConfig {
                        board: self.def.name.clone(),
                        board_def: (BoardDef::builtin(&self.def.name).as_ref() != Some(&self.def))
                            .then(|| Box::new(self.def.clone())),
                        seed: 0,
                        quantum_ps: self.quantum_ns * 1000,
                    },
                    entries: Vec::new(),
                    end_ps: self.now_ps().0,
                    trace_hash: self.trace.hash_string(),
                    journal: self.journal.clone(),
                    project: None,
                })))
            }
            Query::Screenshot { scale } => {
                if !scale.is_finite() || !(0.25..=4.0).contains(scale) {
                    return Err(ApiError::invalid_argument("scale must be within 0.25..=4"));
                }
                board_zybo::render_png(&self.def, &self.board, *scale)
                    .map(Response::Png)
                    .map_err(ApiError::internal)
            }
            Query::VideoFrame => {
                if !self.pl.has_media() {
                    return Err(self.unsupported("video_frame", "no HDMI model in this PL"));
                }
                match &self.media.frame {
                    None => Err(ApiError::not_found(
                        "no complete HDMI frame yet: enable the scan-out and run the simulation",
                    )),
                    Some((w, h, frame, rgb)) => {
                        let rgba: Vec<u8> = rgb
                            .chunks_exact(3)
                            .flat_map(|p| [p[0], p[1], p[2], 255])
                            .collect();
                        board_zybo::encode_rgba_png(*w, *h, rgba)
                            .map(|png| Response::Video {
                                width: *w,
                                height: *h,
                                frame: *frame,
                                png,
                            })
                            .map_err(ApiError::internal)
                    }
                }
            }
            Query::AudioCapture { max_frames } => {
                if !self.pl.has_media() {
                    return Err(self.unsupported("audio_capture", "no codec model in this PL"));
                }
                let end = self.media.audio_first + (self.media.audio.len() / 2) as u64;
                let take = if *max_frames == 0 {
                    u64::MAX
                } else {
                    u64::from(*max_frames)
                };
                let from = end.saturating_sub(take);
                let r = self.media.slice(from, usize::MAX);
                if r.frames == 0 {
                    return Err(ApiError::not_found(
                        "no audio yet: initialise the codec, enable I2S and run the simulation",
                    ));
                }
                Ok(r.into_response())
            }
            Query::AudioFrom {
                from_frame,
                max_frames,
            } => {
                if !self.pl.has_media() {
                    return Err(self.unsupported("audio_from", "no codec model in this PL"));
                }
                Ok(self
                    .media
                    .slice(*from_frame, (*max_frames).max(1) as usize)
                    .into_response())
            }
            Query::Disasm {
                addr,
                count,
                thumb,
            } => {
                if *count == 0 || *count > 256 {
                    return Err(ApiError::invalid_argument(
                        "count must be between 1 and 256",
                    ));
                }
                let a = u32::try_from(*addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                // Thumb by default only when core 0 is stopped in Thumb state.
                let thumb = thumb.unwrap_or_else(|| {
                    self.debug.as_ref().is_some_and(|d| d.stopped.is_some())
                        && self
                            .machine
                            .debug_regs(0)
                            .is_ok_and(|r| r.cpsr & (1 << 5) != 0)
                });
                let bytes = self
                    .machine
                    .read_memory(u64::from(a), (*count as usize) * 4)
                    .map_err(|e| ApiError::io(e.to_string()))?;
                let lines = crate::disasm::disassemble(&bytes, a, thumb, *count as usize)
                    .into_iter()
                    .map(|l| DisasmLine {
                        addr: u64::from(l.addr),
                        size: l.size,
                        hex: l.hex,
                        text: l.text,
                        symbol: self.symbols.containing(l.addr).map(|(s, off)| {
                            if off == 0 {
                                s.name.clone()
                            } else {
                                format!("{}+{off:#x}", s.name)
                            }
                        }),
                    })
                    .collect();
                Ok(Response::Disasm { thumb, lines })
            }
            Query::DeviceState { port } => {
                let bank = self.bank();
                let dev = bank.get(*port).ok_or_else(|| {
                    ApiError::not_found(format!("no device is attached to Pmod port {port}"))
                })?;
                Ok(Response::DeviceState {
                    port: *port,
                    kind: dev.kind().to_string(),
                    description: dev.describe(),
                    values: dev
                        .values()
                        .into_iter()
                        .map(|v| crate::types::DeviceValueInfo {
                            name: v.name.to_string(),
                            value: v.value,
                            unit: v.unit.to_string(),
                            min: v.min,
                            max: v.max,
                        })
                        .collect(),
                    registers: dev
                        .registers()
                        .into_iter()
                        .map(|r| crate::types::DeviceRegisterInfo {
                            name: r.name,
                            addr: r.addr,
                            value: r.value,
                        })
                        .collect(),
                })
            }
            Query::DeviceImage { port } => {
                let bank = self.bank();
                let dev = bank.get(*port).ok_or_else(|| {
                    ApiError::not_found(format!("no device is attached to Pmod port {port}"))
                })?;
                let img = dev.image().ok_or_else(|| {
                    ApiError::unsupported(format!("`{}` has no display", dev.kind()))
                })?;
                let png = board_zybo::encode_rgba_png(img.width, img.height, img.rgba)
                    .map_err(ApiError::internal)?;
                Ok(Response::DeviceImage {
                    port: *port,
                    kind: dev.kind().to_string(),
                    width: img.width,
                    height: img.height,
                    png,
                    description: dev.describe(),
                })
            }
            Query::Breakpoints => {
                let d = self.debug_needed("breakpoint_list")?;
                Ok(Response::Breakpoints(d.breakpoints.clone()))
            }
            Query::Symbol { query } => {
                let symbols = &self.symbols;
                if symbols.is_empty() {
                    return Err(ApiError::not_found("the ELF has no symbols"));
                }
                let t = query.trim();
                let addr = t
                    .strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok());
                let found = match addr {
                    Some(a) => symbols.containing(a).map(|(s, off)| (s, u64::from(off))),
                    None => symbols.by_name(t).map(|s| (s, 0)),
                };
                let (s, offset) = found
                    .ok_or_else(|| ApiError::not_found(format!("no symbol for `{t}`")))?;
                Ok(Response::Symbol(SymbolInfo {
                    name: s.name.clone(),
                    addr: u64::from(s.addr),
                    size: u64::from(s.size),
                    offset,
                }))
            }
            Query::SourceLine { addr } => {
                let a = u32::try_from(*addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                if self.source.is_empty() {
                    return Err(ApiError::not_found(
                        "the ELF has no line information (build the firmware with debug info, \
                         for example `debug = \"line-tables-only\"`)",
                    ));
                }
                let l = self.source.lookup(a).ok_or_else(|| {
                    ApiError::not_found(format!("no source line for address {a:#x}"))
                })?;
                Ok(Response::SourceLine(SourceLine {
                    addr: *addr,
                    file: l.file.to_string(),
                    line: l.line,
                    column: l.column,
                    function: l.function.map(str::to_string).or_else(|| {
                        self.symbols.containing(a).map(|(s, _)| s.name.clone())
                    }),
                }))
            }
            Query::SourceFile { path } => {
                let (resolved, text) = self
                    .source
                    .read_source(path, &self.source_dirs)
                    .map_err(ApiError::not_found)?;
                Ok(Response::SourceFile {
                    file: path.clone(),
                    resolved: resolved.to_string_lossy().into_owned(),
                    text,
                })
            }
            Query::SourceAddrs { file, line } => {
                let f = self
                    .source
                    .addrs_for(file, *line)
                    .map_err(ApiError::not_found)?;
                Ok(Response::SourceAddrs {
                    file: f.file,
                    line: f.line,
                    addrs: f.addrs.into_iter().map(u64::from).collect(),
                })
            }
            Query::CpuRegs { core } if self.debug.is_some() => {
                if *core > 1 {
                    return Err(ApiError::invalid_argument("core must be 0 or 1"));
                }
                let stopped = self
                    .debug
                    .as_ref()
                    .is_some_and(|d| d.stopped.is_some());
                if !stopped {
                    return Err(self.unsupported(
                        "cpu_regs",
                        "registers can only be read while the guest is stopped at a \
                         breakpoint or watchpoint (set one with breakpoint_set and run)",
                    ));
                }
                let r = self
                    .machine
                    .debug_regs(*core)
                    .map_err(|e| ApiError::io(e.to_string()))?;
                let pc_symbol = self.debug.as_ref().and_then(|d| {
                    d.symbols.containing(r.r[15]).map(|(s, off)| {
                        if off == 0 {
                            s.name.clone()
                        } else {
                            format!("{}+{off:#x}", s.name)
                        }
                    })
                });
                Ok(Response::CpuRegs(CpuRegs {
                    core: *core,
                    r: r.r,
                    cpsr: r.cpsr,
                    mode: cpsr_mode(r.cpsr).to_string(),
                    pc_symbol,
                    pc_source: source_ref(&self.source, r.r[15]),
                }))
            }
            Query::CpuRegs { .. } => Err(self.unsupported(
                "cpu_regs",
                "registers are only readable under the debugger: set `[debug] enabled = true` \
                 in project.toml, set a breakpoint and run until the guest stops (a machine \
                 that is merely paused holds QEMU's global lock and cannot be asked)",
            )),
            Query::MemRead { addr, len } => {
                if *len == 0 || *len > 1 << 20 {
                    return Err(ApiError::invalid_argument(
                        "len must be between 1 byte and 1 MiB",
                    ));
                }
                self.machine
                    .read_memory(*addr, *len as usize)
                    .map(|data| Response::Memory { addr: *addr, data })
                    .map_err(|e| ApiError::io(e.to_string()))
            }
            Query::PlSignal { .. } => Err(self.unsupported(
                "pl_signal_get",
                "signal access into the RTL design is not implemented yet (waveforms: `[rtl] waves = true`)",
            )),
            Query::NetCapture { from, max } => {
                let net = self.net_log("net_capture")?;
                Ok(net.page(*from, (*max).clamp(1, 1000)))
            }
            Query::NetStats => Ok(Response::NetStats(self.net_log("net_stats")?.status())),
            Query::NetPcap => Ok(Response::Pcap(self.net_log("net_pcap")?.pcap())),
        }
    }
}
