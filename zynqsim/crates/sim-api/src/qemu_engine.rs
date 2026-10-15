//! Engine: Xilinx QEMU as the PS; native Rust models or an RTL design in
//! XSim as the PL.
//!
//! Fidelity is **F0 plus instruction-count timing**, and the PL models are
//! transaction level (no clock-cycle timing). Both are stated in the status
//! this engine reports.

use crate::bytelog::ByteLog;
use crate::engine::Engine;
use crate::error::ApiError;
use crate::types::{
    AudioSpec, Capability, Command, Fidelity, Input, MediaStatus, Query, Response, Status,
    TraceFormat, VideoSpec,
};
use backend_qemu::{QemuConfig, QemuError, QemuMachine, RunEnd, UartMode};
use board_zybo::{BoardDef, BoardState};
use pl_api::media::{SAMPLE_RATE, wav_bytes};
use pl_api::{
    AudioSource, BusResult, MasterAccess, MediaEvent, PlTarget, PsMasterPort, PsPort, VideoSource,
};
use pl_native::{MathIp, NativePl, RegisterBlock};
use sim_core::Ps;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;
use trace::{TraceEvent, TraceLog};

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
    /// An RTL design; when set it is the PL and `natives` must be empty.
    pub rtl: Option<RtlSpec>,
    /// Board machine name.
    pub board: String,
    /// Name to report as the PL backend.
    pub pl_name: String,
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

    fn set_board_inputs(&mut self, switches: u8, buttons: u8, now_ns: u64) {
        self.inner.set_board_inputs(switches, buttons, now_ns);
    }

    fn drain_board_outputs(&mut self) -> Vec<pl_api::BoardOutput> {
        self.inner.drain_board_outputs()
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
pub const NATIVE_MODELS: &[&str] = &["my_math_ip"];

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
    trace: TraceLog,
    uart1: ByteLog,
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
    /// Every state-changing request applied so far, run targets normalised to
    /// `RunUntil`. Re-applying it to a fresh engine reproduces this one.
    journal: Vec<Command>,
    journal_bytes: usize,
    /// False once the journal outgrew its limits (snapshots/replay then say so).
    journal_ok: bool,
    snapshots: Vec<Snapshot>,
    /// Engines this one has been rebuilt from (names run directories).
    generation: u32,
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
        let def = BoardDef::builtin(&spec.board)
            .ok_or_else(|| ApiError::not_found(format!("unknown board `{}`", spec.board)))?;
        let pl: Box<dyn PlTarget + Send> = match &spec.rtl {
            Some(rtl) => {
                if !spec.natives.is_empty() {
                    return Err(ApiError::invalid_argument(
                        "native PL models and an RTL design cannot be combined yet",
                    ));
                }
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
                Box::new(backend_rtl_xsim::XsimPl::start(&cfg).map_err(|e| {
                    ApiError::io(e.to_string())
                        .with_hint("see build.log / xsim.out.log in the run directory")
                })?)
            }
            None => {
                let mut native = NativePl::new();
                for n in &spec.natives {
                    let block = model(&n.model).ok_or_else(|| {
                        ApiError::invalid_argument(format!(
                            "unknown native model `{}` for instance `{}`; available: {}",
                            n.model,
                            n.name,
                            NATIVE_MODELS.join(", ")
                        ))
                    })?;
                    if n.gp > 1 {
                        return Err(ApiError::invalid_argument(format!(
                            "instance `{}`: M_AXI_GP{} does not exist",
                            n.name, n.gp
                        )));
                    }
                    native.map(n.gp, n.base, block);
                }
                Box::new(native)
            }
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
        let machine = QemuMachine::spawn(&cfg).map_err(map_err)?;

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
        Ok(QemuEngine {
            board: BoardState::new(&def),
            def,
            machine,
            pl: TracedPl {
                inner: pl,
                events: Vec::new(),
            },
            trace,
            uart1: ByteLog::default(),
            target_ns: 0,
            quantum_ns: spec.quantum_ns,
            pl_name: spec.pl_name.clone(),
            uart_tcp: spec.uart == UartMode::Tcp,
            media: Media::default(),
            exited: false,
            failure: None,
            work_dir: spec.work_dir.clone(),
            pl_finish_noted: false,
            spec: spec.clone(),
            journal: Vec::new(),
            journal_bytes: 0,
            journal_ok: true,
            snapshots: Vec::new(),
            generation: 0,
        })
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

    /// Starts over from time zero with the same firmware and PL design.
    fn reset_in_place(&mut self) -> Result<Response, ApiError> {
        let mut fresh = self.rebuild(&[])?;
        std::mem::swap(self, &mut fresh);
        // `fresh` holds the old machine and is dropped here.
        Ok(Response::Ok)
    }

    fn now_ps(&self) -> Ps {
        Ps(self.machine.now_ns().saturating_mul(1000))
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
        for o in self.pl.drain_board_outputs() {
            for i in 0..self.board.leds.len().min(4) {
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
        while !self.exited && self.machine.now_ns() < self.target_ns {
            let boundary = (self.machine.now_ns() / CHUNK_NS + 1) * CHUNK_NS;
            let t = boundary.min(self.target_ns);
            let end = self.machine.run_until(t, &mut self.pl, WALL_LIMIT);
            self.flush();
            match end {
                Ok(RunEnd::ReachedTime) => {}
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
            v.iter()
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
}

impl Drop for QemuEngine {
    fn drop(&mut self) {
        // Stop QEMU so it releases its log files, then remove the run
        // directory. After a failure the logs are the evidence: keep them.
        self.machine.kill();
        if self.failure.is_none() {
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
            Capability::Snapshot,
            Capability::Replay,
        ];
        if self.pl.has_board_pins() {
            caps.push(Capability::BoardIo);
        }
        if self.pl.has_media() {
            caps.push(Capability::Video);
            caps.push(Capability::Audio);
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
            media: self.pl.has_media().then(|| self.media.status()),
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
            Query::Snapshots => Ok(Response::Snapshots(
                self.snapshots.iter().map(|s| s.name.clone()).collect(),
            )),
            Query::Recording => {
                self.journal_needed("recording")?;
                Ok(Response::Recording(Box::new(crate::Recording {
                    version: 1,
                    config: crate::SimConfig {
                        board: self.def.name.clone(),
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
            Query::CpuRegs { .. } => Err(self.unsupported(
                "cpu_regs",
                "while the machine is paused at a sync QEMU holds its global lock, so its \
                 monitor cannot answer; registers are not observable deterministically",
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
        }
    }
}
