//! Adapter that builds an RTL design with Icarus Verilog (`iverilog`, then `vvp`) and runs it
//! as a co-simulation peer, exposing it to a PS engine as a [`PlTarget`] (ADR-037).
//!
//! It is the third RTL backend, next to `backend-rtl-xsim` and `backend-rtl-verilator`, and
//! speaks the same remote-port protocol. Icarus has no DPI-C, so the testbench's `rp_*`
//! functions are the VPI system functions `$zs_rp_*` that the Rust module `zynqsim.vpi`
//! (crate `vpi-icarus`) registers in `vvp`. That module calls the same code as the DPI
//! archive (`xsim-dpi`), and the testbench is compiled with `-DZS_ICARUS`, which selects the
//! `$zs_` forms (see `sim-api::tbgen`).
//!
//! The plumbing after the start-up (PS accesses, PL master accesses, interrupts, lock-step,
//! pin events, media) is the same as `backend-rtl-verilator`'s, copied, not shared (ADR-027
//! precedent).
//!
//! ## Honest limits
//!
//! * Windows only (the VPI module is the Windows build; see ADR-037).
//! * 4-state, event-driven, like XSim, but Icarus implements less of SystemVerilog: a
//!   construct it rejects is reported with its file and line by [`diag`].
//! * No waveform output yet.

mod diag;
pub mod tools;

pub use diag::{Diagnostic, Severity, errors as diagnostic_errors, parse_messages};
pub use tools::IcarusTools;

use interop::{
    BusAccess, BusCmd, Interrupt, Packet, Session, SessionError, Sync, cap, flags, resp,
};
use pl_api::media::{DEV_AUDIO_IN, DEV_AUDIO_OUT, DEV_CODEC_REG, DEV_VIDEO_IN, DEV_VIDEO_OUT};
use pl_api::protocol::DEV_PROTOCOL_CHECK;
use pl_api::{
    AudioSource, BoardOutput, BusError, BusResult, DEV_BOARD_IN, DEV_BOARD_OUT, DEV_M_AXI_GP0,
    DEV_PIN_IN, DEV_PIN_OUT, DEV_WIRES_PS_TO_PL, MasterAccess, MediaEvent, PinEvent, PinSink,
    PlTarget, ProtocolFinding, PsMasterPort, PsPort, VideoSource,
};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Name of the VPI module inside its directory (`vvp -mzynqsim` loads `zynqsim.vpi`).
const VPI_NAME: &str = "zynqsim";
/// File name the module is copied to in the run directory.
const VPI_FILE: &str = "zynqsim.vpi";
/// The compiled program inside the run directory.
const PROGRAM: &str = "design.vvp";
/// The compile options that shape the build. They are part of the cache key.
const COMPILE_FLAGS: &[&str] = &["-g2012", "-DZS_ICARUS"];
/// Programs kept in the build cache.
const CACHE_KEEP: usize = 6;

/// How to build and run a design with Icarus Verilog.
#[derive(Clone, Debug)]
pub struct IcarusConfig {
    /// `iverilog.exe`, the compiler.
    pub iverilog: PathBuf,
    /// `vvp.exe`, the runtime.
    pub vvp: PathBuf,
    /// The VPI module, `zynqsim.vpi` (built by `cargo xtask vpi`).
    pub vpi: PathBuf,
    /// Directories put first on `PATH` for the tools: the DLL folders of Icarus
    /// (`bin` and `lib` of the suite). The search also uses the inherited `PATH`.
    pub tool_dirs: Vec<PathBuf>,
    /// Verilog / SystemVerilog sources, testbench included.
    pub sources: Vec<PathBuf>,
    /// Top module (the testbench).
    pub top: String,
    /// Preprocessor defines (`NAME` or `NAME=VALUE`).
    pub defines: Vec<String>,
    /// Scratch directory (program, logs).
    pub work_dir: PathBuf,
    /// How long to wait for the design to connect after it starts.
    pub connect_timeout: Duration,
    /// Where compiled programs are kept between runs. A design with the same sources,
    /// defines, top, module and Icarus version is not compiled again.
    pub cache_dir: Option<PathBuf>,
}

/// Errors from the Icarus adapter.
#[derive(Debug)]
pub enum IcarusError {
    /// A configured file does not exist.
    MissingFile(PathBuf),
    /// Icarus or the build failed; the messages and the log tail are included.
    Build {
        /// Which step failed.
        step: &'static str,
        /// The `error` messages (file, line, construct).
        diagnostics: Vec<Diagnostic>,
        /// Last lines of the build log.
        log: String,
    },
    /// The host or the platform cannot run this adapter.
    Unsupported(String),
    /// An OS error.
    Io(std::io::Error),
    /// The simulation exited, or never connected.
    NotConnected {
        /// Why, with the tail of the simulator output.
        detail: String,
    },
    /// The remote-port session failed.
    Rp(SessionError),
}

impl fmt::Display for IcarusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IcarusError::MissingFile(p) => write!(f, "file not found: {}", p.display()),
            IcarusError::Build {
                step,
                diagnostics,
                log,
            } => {
                write!(f, "Icarus Verilog build failed at {step}:\n{log}")?;
                if !diagnostics.is_empty() {
                    write!(f, "\nunsupported or invalid constructs:")?;
                    for d in diagnostics {
                        match (&d.file, d.line) {
                            (Some(file), Some(line)) => {
                                write!(f, "\n  {file}:{line}: {}", d.message)?;
                            }
                            _ => write!(f, "\n  {}", d.message)?,
                        }
                    }
                }
                Ok(())
            }
            IcarusError::Unsupported(why) => write!(f, "Icarus adapter: {why}"),
            IcarusError::Io(e) => write!(f, "Icarus adapter I/O error: {e}"),
            IcarusError::NotConnected { detail } => {
                write!(f, "the simulation did not connect to the adapter: {detail}")
            }
            IcarusError::Rp(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for IcarusError {}

impl From<std::io::Error> for IcarusError {
    fn from(e: std::io::Error) -> Self {
        IcarusError::Io(e)
    }
}

impl From<SessionError> for IcarusError {
    fn from(e: SessionError) -> Self {
        IcarusError::Rp(e)
    }
}

/// Counters for tests and the status view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IcarusStats {
    /// PS reads served by the design.
    pub ps_reads: u64,
    /// PS writes served by the design.
    pub ps_writes: u64,
    /// PL master reads (DMA) forwarded to the PS.
    pub pl_reads: u64,
    /// PL master writes forwarded to the PS.
    pub pl_writes: u64,
    /// Interrupt edges forwarded to the PS.
    pub irqs: u64,
    /// Lock-step syncs completed.
    pub syncs: u64,
}

/// What a PS access asks of the PL.
enum PsRequest<'a> {
    Read { len: usize },
    Write(&'a [u8]),
}

/// A running Icarus simulation.
pub struct IcarusPl {
    child: Child,
    rp: Session,
    finished: bool,
    stats: IcarusStats,
    master_log: Vec<MasterAccess>,
    board_out: Vec<BoardOutput>,
    /// AXI protocol-checker findings of the testbench (ADR-025).
    protocol: Vec<ProtocolFinding>,
    media: Vec<MediaEvent>,
    video_src: VideoSource,
    audio_src: AudioSource,
    /// The design uses the codec or HDMI DPI functions.
    has_media: bool,
    /// The design uses the board switch/button/LED DPI functions.
    has_pins: bool,
    /// The design uses the Pmod/RGB pin-group DPI functions.
    has_pin_groups: bool,
    pin_events: Vec<PinEvent>,
    /// The devices on the Pmod ports, asked as the pin events arrive.
    pin_sink: Option<Box<dyn PinSink>>,
    work_dir: PathBuf,
    warnings: Vec<Diagnostic>,
    now_ns: u64,
}

/// Whether any of `sources` mentions one of `needles`. A design that does not is not
/// offered the matching board features.
fn sources_mention(sources: &[PathBuf], needles: &[&str]) -> bool {
    sources.iter().any(|p| {
        fs::read_to_string(p)
            .map(|t| needles.iter().any(|n| t.contains(n)))
            .unwrap_or(false)
    })
}

/// Content key of a build: everything that changes the compiled program.
fn build_key(cfg: &IcarusConfig, vpi: &[u8], version: &str) -> std::io::Result<String> {
    fn fnv(h: &mut u64, bytes: &[u8]) {
        for b in bytes {
            *h ^= u64::from(*b);
            *h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        // Separate fields so that moving bytes between them changes the key.
        *h ^= 0xFF;
        *h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let mut a = 0xCBF2_9CE4_8422_2325u64;
    let mut b = 0x8422_2325_CBF2_9CE4u64;
    let mut feed = |bytes: &[u8]| {
        fnv(&mut a, bytes);
        fnv(&mut b, bytes);
    };
    feed(b"zynqsim-icarus-build-v1");
    feed(version.as_bytes());
    feed(cfg.top.as_bytes());
    for f in COMPILE_FLAGS {
        feed(f.as_bytes());
    }
    for d in &cfg.defines {
        feed(d.as_bytes());
    }
    feed(vpi);
    for s in &cfg.sources {
        feed(
            s.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .as_bytes(),
        );
        feed(&fs::read(s)?);
    }
    Ok(format!("{a:016x}{b:016x}"))
}

/// Removes the oldest cache entries beyond [`CACHE_KEEP`].
fn prune_cache(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    entries.sort();
    while entries.len() > CACHE_KEEP {
        let (_, p) = entries.remove(0);
        let _ = fs::remove_dir_all(p);
    }
}

fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(25)..].join("\n")
}

/// Forward slashes, for paths given to the tools on their command lines.
fn fwd(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// The search path of the tools: the configured tool directories, then the inherited `PATH`.
fn build_path(cfg: &IcarusConfig) -> std::io::Result<OsString> {
    let mut dirs = cfg.tool_dirs.clone();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

/// The Icarus version line, which is part of the cache key.
fn iverilog_version(cfg: &IcarusConfig, path: &OsStr) -> Result<String, IcarusError> {
    let mut cmd = Command::new(&cfg.iverilog);
    cmd.arg("-V").env("PATH", path).stdin(Stdio::null());
    hide_window(&mut cmd);
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(IcarusError::Build {
            step: "iverilog -V",
            diagnostics: Vec::new(),
            log: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

fn first8(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    u64::from_le_bytes(a)
}

fn master_port(dev: u32) -> Option<PsMasterPort> {
    Some(match dev {
        0 => PsMasterPort::SAxiGp0,
        1 => PsMasterPort::SAxiGp1,
        2 => PsMasterPort::SAxiHp0,
        3 => PsMasterPort::SAxiHp1,
        4 => PsMasterPort::SAxiHp2,
        5 => PsMasterPort::SAxiHp3,
        6 => PsMasterPort::SAxiAcp,
        _ => return None,
    })
}

impl IcarusPl {
    /// Compiles the design (or reuses its cached program), starts it and completes the
    /// handshake.
    pub fn start(cfg: &IcarusConfig) -> Result<IcarusPl, IcarusError> {
        if !cfg!(windows) {
            return Err(IcarusError::Unsupported(
                "the zynqsim VPI module is built for Windows only for now".to_string(),
            ));
        }
        for p in cfg.sources.iter().chain([&cfg.vpi, &cfg.iverilog, &cfg.vvp]) {
            if !p.is_file() {
                return Err(IcarusError::MissingFile(p.clone()));
            }
        }
        fs::create_dir_all(&cfg.work_dir)?;
        let work = fs::canonicalize(&cfg.work_dir)?;
        // `canonicalize` yields `\\?\` paths, which the tools do not take.
        let work = PathBuf::from(work.display().to_string().trim_start_matches(r"\\?\"));

        // The module is copied under its loadable name, in a folder of its own.
        let vpi_dir = work.join("vpi");
        fs::create_dir_all(&vpi_dir)?;
        let vpi_copy = vpi_dir.join(VPI_FILE);
        fs::copy(&cfg.vpi, &vpi_copy)?;
        let vpi_bytes = fs::read(&vpi_copy)?;

        let path = build_path(cfg)?;
        let version = iverilog_version(cfg, &path)?;
        let key = match &cfg.cache_dir {
            Some(_) => Some(build_key(cfg, &vpi_bytes, &version)?),
            None => None,
        };
        let build_log = work.join("build.log");
        let program = work.join(PROGRAM);

        let cached = match (&cfg.cache_dir, &key) {
            (Some(dir), Some(k)) => {
                let entry = dir.join(k);
                if entry.join("ok").is_file() && fs::copy(entry.join(PROGRAM), &program).is_ok() {
                    let _ = fs::copy(entry.join("build.log"), &build_log);
                    // Touch so that pruning keeps recently used builds.
                    let _ = fs::write(entry.join("ok"), b"ok");
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !cached {
            let _ = fs::remove_file(&program);
            let mut cmd = Command::new(&cfg.iverilog);
            cmd.current_dir(&work)
                .args(COMPILE_FLAGS)
                .args(["-m", VPI_NAME, "-s"])
                .arg(&cfg.top)
                .args(["-o", PROGRAM]);
            for d in &cfg.defines {
                cmd.arg(format!("-D{d}"));
            }
            for s in &cfg.sources {
                cmd.arg(fwd(s));
            }
            // The compiler loads the module at compile time, so that the system functions
            // have their widths (a 64-bit result is 64 bits wide only then).
            cmd.env("PATH", &path)
                .env("IVERILOG_VPI_MODULE_PATH", fwd(&vpi_dir))
                .stdin(Stdio::null());
            let out = fs::File::create(&build_log)?;
            cmd.stdout(out.try_clone()?).stderr(out);
            hide_window(&mut cmd);
            let status = cmd.status()?;
            let log = fs::read_to_string(&build_log).unwrap_or_default();
            if !status.success() || !program.is_file() {
                return Err(IcarusError::Build {
                    step: "iverilog (compile)",
                    diagnostics: diag::errors(&log),
                    log: tail(&build_log),
                });
            }
            if let (Some(dir), Some(k)) = (&cfg.cache_dir, &key) {
                // Best effort: a cache that cannot be written is not an error.
                let tmp = dir.join(format!("{k}.tmp{}", std::process::id()));
                let entry = dir.join(k);
                let stored = fs::create_dir_all(&tmp)
                    .and_then(|()| fs::copy(&program, tmp.join(PROGRAM)))
                    .and_then(|_| fs::copy(&build_log, tmp.join("build.log")))
                    .and_then(|_| fs::write(tmp.join("ok"), b"ok"))
                    .and_then(|()| {
                        let _ = fs::remove_dir_all(&entry);
                        fs::rename(&tmp, &entry)
                    });
                if stored.is_err() {
                    let _ = fs::remove_dir_all(&tmp);
                } else {
                    prune_cache(dir);
                }
            }
        }
        let warnings: Vec<Diagnostic> =
            parse_messages(&fs::read_to_string(&build_log).unwrap_or_default())
                .into_iter()
                .filter(|d| !d.is_error())
                .collect();

        // ---- run ----------------------------------------------------------------------
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let out = work.join("sim.out.log");
        let mut cmd = Command::new(&cfg.vvp);
        cmd.current_dir(&work)
            .arg("-M")
            .arg(fwd(&vpi_dir))
            .arg(format!("-m{VPI_NAME}"))
            .arg(PROGRAM)
            .env("PATH", &path)
            .env("RP_SOCKET", format!("tcp:127.0.0.1:{port}"))
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out)?)
            .stderr(fs::File::create(work.join("sim.err.log"))?);
        hide_window(&mut cmd);
        let mut child = cmd.spawn()?;

        // ---- accept the DPI connection -------------------------------------------------
        listener.set_nonblocking(true)?;
        let start = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(Some(st)) = child.try_wait() {
                        return Err(IcarusError::NotConnected {
                            detail: format!("simulation exited ({st}); output:\n{}", tail(&out)),
                        });
                    }
                    if start.elapsed() > cfg.connect_timeout {
                        kill_tree(&mut child);
                        return Err(IcarusError::NotConnected {
                            detail: format!(
                                "no connection within {:?}; output:\n{}",
                                cfg.connect_timeout,
                                tail(&out)
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    kill_tree(&mut child);
                    return Err(e.into());
                }
            }
        };
        stream.set_nonblocking(false)?;
        let mut rp = Session::from_stream(stream)?;
        rp.send_hello(&[cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES])?;
        // The module's hello arrives first; consuming it records its capabilities.
        match rp.recv()? {
            Packet::Hello { .. } => {}
            p => {
                kill_tree(&mut child);
                return Err(IcarusError::NotConnected {
                    detail: format!("expected hello, got {p:?}"),
                });
            }
        }

        let sources = &cfg.sources;
        Ok(IcarusPl {
            child,
            rp,
            finished: false,
            stats: IcarusStats::default(),
            master_log: Vec::new(),
            board_out: Vec::new(),
            protocol: Vec::new(),
            media: Vec::new(),
            video_src: VideoSource::color_bars(320, 240),
            audio_src: AudioSource::Silence,
            has_media: sources_mention(
                sources,
                &["zs_ssm2603", "zs_hdmi", "zs_audio_", "zs_video_"],
            ),
            has_pins: sources_mention(sources, &["tel_ctrl", "tel_event"]),
            has_pin_groups: sources_mention(sources, &["zs_pin_"]),
            pin_events: Vec::new(),
            pin_sink: None,
            work_dir: work,
            warnings,
            now_ns: 0,
        })
    }

    /// Counters.
    pub fn stats(&self) -> IcarusStats {
        self.stats
    }

    /// The scratch directory holding logs and products.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Icarus's warnings for the design (for example a construct it accepts with a note;
    /// they do not stop the build).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// Stops the simulation and everything it spawned.
    pub fn kill(&mut self) {
        kill_tree(&mut self.child);
        self.finished = true;
    }

    /// Handles one packet the design sent while we wait for something else.
    fn serve(&mut self, pkt: Packet, ps: &mut dyn PsPort) -> Result<(), BusError> {
        match pkt {
            Packet::Bus(b) if !b.is_response() && self.serve_media(&b) => {}
            Packet::Bus(b) if !b.is_response() => {
                self.serve_master(b, ps);
            }
            Packet::Interrupt(i) if i.dev == DEV_VIDEO_OUT => {
                self.media.push(MediaEvent::VideoFrameEnd {
                    frame: i.vector as u32,
                    t_ns: i.timestamp,
                });
            }
            Packet::Interrupt(i) if i.dev == DEV_CODEC_REG => {
                self.media.push(MediaEvent::CodecRegister {
                    t_ns: i.timestamp,
                    reg: i.line as u8,
                    value: (i.vector & 0x1FF) as u16,
                });
            }
            Packet::Interrupt(i) if i.dev == DEV_PROTOCOL_CHECK => {
                self.protocol.push(ProtocolFinding::from_packet(
                    i.timestamp,
                    i.line,
                    i.val,
                    i.vector,
                ));
            }
            Packet::Interrupt(i) if i.dev == DEV_PIN_OUT => {
                self.pin_events.push(PinEvent {
                    t_ns: i.timestamp,
                    group: i.line as u8,
                    levels: i.val,
                });
                // A report only: the devices see it, nothing is answered.
                if let Some(sink) = &mut self.pin_sink {
                    sink.pin_change(i.line as u8, i.timestamp, i.val);
                }
            }
            Packet::Interrupt(i) if i.dev == DEV_BOARD_OUT => {
                self.board_out.push(BoardOutput {
                    t_ns: i.timestamp,
                    leds: i.val,
                });
            }
            Packet::Interrupt(i) if i.flags & flags::RESPONSE == 0 => {
                self.stats.irqs += 1;
                let r = ps.irq(i.line, i.val != 0, i.timestamp);
                if i.flags & flags::POSTED == 0 {
                    let _ = self.rp.send(&Packet::Interrupt(Interrupt {
                        flags: i.flags | flags::RESPONSE,
                        ..i
                    }));
                }
                r?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Media traffic from the testbench's HDMI and codec models. Returns true when the packet
    /// was one of theirs.
    fn serve_media(&mut self, b: &BusAccess) -> bool {
        match (b.dev, b.cmd) {
            (DEV_VIDEO_OUT, BusCmd::Write) => {
                self.media.push(MediaEvent::VideoLine {
                    frame: (b.addr >> 16) as u32,
                    y: (b.addr & 0xFFFF) as u32,
                    t_ns: b.timestamp,
                    rgb: b.data.clone(),
                });
                true
            }
            (DEV_AUDIO_OUT, BusCmd::Write) => {
                self.media.push(MediaEvent::Audio {
                    first_frame: b.addr,
                    t_ns: b.timestamp,
                    samples: b
                        .data
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect(),
                });
                true
            }
            (DEV_VIDEO_IN, BusCmd::Read) => {
                let y = (b.addr & 0xFFFF) as u32;
                let (w, h) = (b.len / 3, b.width);
                // A board device that drives the video input (a camera) first.
                let from_device = self
                    .pin_sink
                    .as_mut()
                    .and_then(|s| s.video_line((b.addr >> 16) as u32, y, w, h))
                    .filter(|d| d.len() == (w * 3) as usize);
                let data = from_device.unwrap_or_else(|| self.video_src.line(y, w, h));
                let rsp = b.response(resp::OK, data, b.timestamp);
                let _ = self.rp.send(&Packet::Bus(rsp));
                true
            }
            (DEV_PIN_OUT, BusCmd::Read) => {
                // The synchronous exchange: report the change, answer at once with what the
                // devices drive (1 = released).
                let (group, levels) = ((b.addr & 0xFF) as u8, ((b.addr >> 8) & 0xFF) as u8);
                self.pin_events.push(PinEvent {
                    t_ns: b.timestamp,
                    group,
                    levels,
                });
                let answer = match &mut self.pin_sink {
                    Some(sink) => sink.pin_change(group, b.timestamp, levels),
                    None => 0xFF,
                };
                let rsp = b.response(resp::OK, vec![answer], b.timestamp);
                let _ = self.rp.send(&Packet::Bus(rsp));
                true
            }
            (DEV_AUDIO_IN, BusCmd::Read) => {
                let samples = self.audio_src.block(b.addr, (b.len / 4) as usize);
                let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                let rsp = b.response(resp::OK, data, b.timestamp);
                let _ = self.rp.send(&Packet::Bus(rsp));
                true
            }
            _ => false,
        }
    }

    /// A PL master access: forward it to the PS and answer the design.
    fn serve_master(&mut self, b: BusAccess, ps: &mut dyn PsPort) {
        let len = b.len as usize;
        let code = match master_port(b.dev) {
            None => resp::ADDR_ERROR,
            Some(port) => {
                let r = match b.cmd {
                    BusCmd::Read => {
                        let mut buf = vec![0u8; len];
                        ps.mem_read(port, b.addr, &mut buf).map(|()| buf)
                    }
                    BusCmd::Write => ps.mem_write(port, b.addr, &b.data).map(|()| Vec::new()),
                };
                let shown: &[u8] = if b.cmd == BusCmd::Read {
                    r.as_deref().unwrap_or(&[])
                } else {
                    &b.data
                };
                self.master_log.push(MasterAccess {
                    t_ns: b.timestamp,
                    port,
                    write: b.cmd == BusCmd::Write,
                    addr: b.addr,
                    len: b.len,
                    data: first8(shown),
                    ok: r.is_ok(),
                });
                match b.cmd {
                    BusCmd::Read => self.stats.pl_reads += 1,
                    BusCmd::Write => self.stats.pl_writes += 1,
                }
                match r {
                    Ok(d) => {
                        if !b.is_posted() {
                            let rsp = b.response(resp::OK, d, b.timestamp);
                            let _ = self.rp.send(&Packet::Bus(rsp));
                        }
                        return;
                    }
                    Err(BusError::Decode) => resp::ADDR_ERROR,
                    Err(BusError::Slave) => resp::BUS_GENERIC_ERROR,
                }
            }
        };
        if !b.is_posted() {
            let rsp = b.response(code, Vec::new(), b.timestamp);
            let _ = self.rp.send(&Packet::Bus(rsp));
        }
    }

    /// Sends a PS access and waits for its response, serving everything the design sends
    /// in the meantime.
    fn ps_access(
        &mut self,
        req: PsRequest<'_>,
        gp: u8,
        addr: u64,
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<Vec<u8>> {
        let (cmd, len, data) = match req {
            PsRequest::Read { len } => (BusCmd::Read, len, &[][..]),
            PsRequest::Write(d) => (BusCmd::Write, d.len(), d),
        };
        if self.finished {
            return Err(BusError::Slave);
        }
        let id = self.rp.alloc_id();
        let req = BusAccess {
            cmd,
            id,
            flags: 0,
            dev: DEV_M_AXI_GP0 + u32::from(gp),
            timestamp: now_ns,
            attributes: 0,
            addr,
            len: len as u32,
            width: 0,
            stream_width: len as u32,
            master_id: 0,
            data: data.to_vec(),
            byte_enable: Vec::new(),
            ext: self.rp.peer_ext_base(),
        };
        if self.rp.send(&Packet::Bus(req)).is_err() {
            self.finished = true;
            return Err(BusError::Slave);
        }
        loop {
            match self.rp.recv() {
                Ok(Packet::Bus(r)) if r.is_response() && r.id == id => {
                    return match r.response_code() {
                        resp::OK => Ok(r.data),
                        resp::ADDR_ERROR => Err(BusError::Decode),
                        _ => Err(BusError::Slave),
                    };
                }
                Ok(other) => self.serve(other, ps)?,
                Err(_) => {
                    // The design ended (testbench `$finish`) or died.
                    self.finished = true;
                    return Err(BusError::Slave);
                }
            }
        }
    }
}

impl PlTarget for IcarusPl {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.now_ns = self.now_ns.max(now_ns);
        let d = self.ps_access(PsRequest::Read { len: buf.len() }, gp, addr, now_ns, ps)?;
        self.stats.ps_reads += 1;
        if d.len() != buf.len() {
            return Err(BusError::Slave);
        }
        buf.copy_from_slice(&d);
        Ok(())
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.now_ns = self.now_ns.max(now_ns);
        self.ps_access(PsRequest::Write(data), gp, addr, now_ns, ps)?;
        self.stats.ps_writes += 1;
        Ok(())
    }

    fn wire(&mut self, line: u32, level: bool, now_ns: u64) {
        if self.finished {
            return;
        }
        let id = self.rp.alloc_id();
        let _ = self.rp.send(&Packet::Interrupt(Interrupt {
            id,
            flags: flags::POSTED,
            dev: DEV_WIRES_PS_TO_PL,
            timestamp: now_ns,
            vector: 0,
            line,
            val: u8::from(level),
        }));
    }

    fn advance(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        if self.finished || now_ns <= self.now_ns {
            return;
        }
        self.now_ns = now_ns;
        let id = self.rp.alloc_id();
        if self
            .rp
            .send(&Packet::Sync(Sync {
                id,
                flags: 0,
                dev: DEV_M_AXI_GP0,
                timestamp: now_ns,
            }))
            .is_err()
        {
            self.finished = true;
            return;
        }
        loop {
            match self.rp.recv() {
                Ok(Packet::Sync(s)) if s.flags & flags::RESPONSE != 0 && s.id == id => {
                    self.stats.syncs += 1;
                    return;
                }
                Ok(other) => {
                    // An interrupt the PS rejected is not an error here: keep waiting for the sync.
                    let _ = self.serve(other, ps);
                }
                Err(_) => {
                    self.finished = true;
                    return;
                }
            }
        }
    }

    fn drain_master_log(&mut self) -> Vec<MasterAccess> {
        std::mem::take(&mut self.master_log)
    }

    fn has_media(&self) -> bool {
        self.has_media
    }

    fn drain_media(&mut self) -> Vec<MediaEvent> {
        std::mem::take(&mut self.media)
    }

    fn set_media_sources(&mut self, video: Option<VideoSource>, audio: Option<AudioSource>) {
        if let Some(v) = video {
            self.video_src = v;
        }
        if let Some(a) = audio {
            self.audio_src = a;
        }
    }

    fn has_board_pins(&self) -> bool {
        self.has_pins
    }

    fn has_pin_groups(&self) -> bool {
        self.has_pin_groups
    }

    fn drain_pin_events(&mut self) -> Vec<PinEvent> {
        std::mem::take(&mut self.pin_events)
    }

    fn drain_protocol_findings(&mut self) -> Vec<ProtocolFinding> {
        std::mem::take(&mut self.protocol)
    }

    fn set_pin_sink(&mut self, sink: Box<dyn PinSink>) {
        self.pin_sink = Some(sink);
    }

    fn set_pin_inputs(&mut self, group: u8, levels: u8, now_ns: u64) {
        if self.finished {
            return;
        }
        let id = self.rp.alloc_id();
        let _ = self.rp.send(&Packet::Interrupt(Interrupt {
            id,
            flags: flags::POSTED,
            dev: DEV_PIN_IN,
            timestamp: now_ns,
            vector: 0,
            line: u32::from(group),
            val: levels,
        }));
    }

    fn set_board_inputs(&mut self, switches: u8, buttons: u8, now_ns: u64) {
        if self.finished {
            return;
        }
        for (line, val) in [(0u32, switches), (1, buttons)] {
            let id = self.rp.alloc_id();
            let _ = self.rp.send(&Packet::Interrupt(Interrupt {
                id,
                flags: flags::POSTED,
                dev: DEV_BOARD_IN,
                timestamp: now_ns,
                vector: 0,
                line,
                val,
            }));
        }
    }

    fn drain_board_outputs(&mut self) -> Vec<BoardOutput> {
        std::mem::take(&mut self.board_out)
    }

    fn finished(&self) -> bool {
        self.finished
    }
}

impl Drop for IcarusPl {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

/// Kills the process and its children.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut c);
        let _ = c.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
