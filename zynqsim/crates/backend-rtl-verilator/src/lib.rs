//! Adapter that builds an RTL design with Verilator (`--binary --timing`) and runs it as a
//! co-simulation peer, exposing it to a PS engine as a [`PlTarget`].
//!
//! It is the second RTL backend next to `backend-rtl-xsim` and speaks the same protocol:
//! the testbench imports the `rp_*` DPI functions, `xsim-dpi` (the same static library,
//! `target/dpi/release/libxsim_dpi.a`) implements them and connects as a remote-port client
//! to this adapter, which forwards PS accesses, PL master accesses, interrupts and the
//! lock-step syncs exactly as the XSim adapter does.
//!
//! ## How the build works
//!
//! Verilator translates the sources to C++ and compiles them with the host's C++ compiler
//! and GNU make (`make -C obj -f ...`). No C/C++ is written here: the model is Verilator's
//! output and the DPI library is the Rust archive. The DPI archive is linked with
//! `-LDFLAGS`; on Windows two extra flags are needed: `-static` (the executable then needs
//! no MinGW DLLs at run time) and `--allow-multiple-definition` (the Rust archive and
//! libgcc both define `_Unwind_Resume`; the archive's definition is never reached because
//! nothing unwinds). The finished executable is cached by content (see [`VerilatorConfig::cache_dir`]).
//!
//! ## Honest limits
//!
//! * Windows only for now (the DPI archive is the Windows build of `xsim-dpi`).
//! * Needs `make`, a POSIX `sh` (Git for Windows ships one) and a C++ compiler on the
//!   search path; [`VerilatorError::MissingTool`] says which one is missing.
//! * Two-state simulation: no X/Z, and only what Verilator accepts (its messages are
//!   returned as [`Diagnostic`]s). Timing and event semantics differ in detail from XSim.
//! * No waveform output yet.

mod diag;
pub mod tools;

pub use diag::{Diagnostic, Severity, errors as diagnostic_errors, parse_messages};
pub use tools::VerilatorTools;

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

/// File name of the DPI archive inside the run directory.
const DPI_COPY: &str = "libxsim_dpi.a";
/// Name of the built executable (`obj/zs_sim.exe`).
const SIM_NAME: &str = "zs_sim";
/// Options that shape the build. They are part of the cache key.
const BUILD_FLAGS: &[&str] = &[
    "--binary",
    "--timing",
    "-Wno-fatal",
    "-j",
    "0",
    "-LDFLAGS",
    "-static",
    "-LDFLAGS",
    "-Wl,--allow-multiple-definition",
];
/// Snapshots kept in the build cache.
const CACHE_KEEP: usize = 6;

/// How to build and run a design with Verilator.
#[derive(Clone, Debug)]
pub struct VerilatorConfig {
    /// The Verilator driver: `verilator_bin.exe` (the C++ binary; the `verilator` launcher
    /// is a Perl script that needs Perl).
    pub verilator: PathBuf,
    /// `VERILATOR_ROOT` (the `share/verilator` directory), if known.
    pub root: Option<PathBuf>,
    /// Directories put first on `PATH` for the build: the directory of `make`, of `sh`, of
    /// the C++ compiler and of Verilator's `python3`. The search also uses the inherited `PATH`.
    pub tool_dirs: Vec<PathBuf>,
    /// Verilog / SystemVerilog sources, testbench included.
    pub sources: Vec<PathBuf>,
    /// Top module (the testbench).
    pub top: String,
    /// Preprocessor defines (`NAME` or `NAME=VALUE`).
    pub defines: Vec<String>,
    /// The freestanding `libxsim_dpi.a`.
    pub dpi_lib: PathBuf,
    /// Scratch directory (build products, logs).
    pub work_dir: PathBuf,
    /// How long to wait for the design to connect after it starts.
    pub connect_timeout: Duration,
    /// Where built executables are kept between runs. A design with the same sources,
    /// defines, top, DPI library and Verilator version is not translated again.
    pub cache_dir: Option<PathBuf>,
}

/// Errors from the Verilator adapter.
#[derive(Debug)]
pub enum VerilatorError {
    /// A configured file does not exist.
    MissingFile(PathBuf),
    /// A program the build needs is not on the search path.
    MissingTool {
        /// `make`, `g++` or `sh`.
        tool: &'static str,
        /// How to get it.
        hint: &'static str,
    },
    /// Verilator or the build failed; the messages and the log tail are included.
    Build {
        /// Which step failed.
        step: &'static str,
        /// The `%Error` messages (file, line, construct).
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

impl fmt::Display for VerilatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerilatorError::MissingFile(p) => write!(f, "file not found: {}", p.display()),
            VerilatorError::MissingTool { tool, hint } => {
                write!(f, "`{tool}` was not found on the build path: {hint}")
            }
            VerilatorError::Build {
                step,
                diagnostics,
                log,
            } => {
                write!(f, "Verilator build failed at {step}:\n{log}")?;
                if !diagnostics.is_empty() {
                    write!(f, "\nunsupported or invalid constructs:")?;
                    for d in diagnostics {
                        match (&d.file, d.line) {
                            (Some(file), Some(line)) => {
                                write!(f, "\n  {file}:{line}: `{}`", d.construct)?;
                            }
                            _ => write!(f, "\n  `{}`", d.construct)?,
                        }
                        write!(f, " ({})", d.message)?;
                    }
                }
                Ok(())
            }
            VerilatorError::Unsupported(why) => write!(f, "Verilator adapter: {why}"),
            VerilatorError::Io(e) => write!(f, "Verilator adapter I/O error: {e}"),
            VerilatorError::NotConnected { detail } => {
                write!(f, "the simulation did not connect to the adapter: {detail}")
            }
            VerilatorError::Rp(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for VerilatorError {}

impl From<std::io::Error> for VerilatorError {
    fn from(e: std::io::Error) -> Self {
        VerilatorError::Io(e)
    }
}

impl From<SessionError> for VerilatorError {
    fn from(e: SessionError) -> Self {
        VerilatorError::Rp(e)
    }
}

/// Counters for tests and the status view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerilatorStats {
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

/// A running Verilator simulation.
pub struct VerilatorPl {
    child: Child,
    rp: Session,
    finished: bool,
    stats: VerilatorStats,
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

/// Content key of a build: everything that changes the translated and compiled model.
fn build_key(cfg: &VerilatorConfig, dpi_archive: &[u8], version: &str) -> std::io::Result<String> {
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
    feed(b"zynqsim-verilator-build-v1");
    feed(version.as_bytes());
    feed(
        cfg.root
            .as_ref()
            .map(|r| r.display().to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    feed(cfg.top.as_bytes());
    for f in BUILD_FLAGS {
        feed(f.as_bytes());
    }
    for d in &cfg.defines {
        feed(d.as_bytes());
    }
    feed(dpi_archive);
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

/// Forward slashes: Verilator's makefiles mishandle `\` in the paths they are given.
fn fwd(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// The search path of the build: the configured tool directories, then the inherited `PATH`.
fn build_path(cfg: &VerilatorConfig) -> std::io::Result<OsString> {
    let mut dirs = cfg.tool_dirs.clone();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

/// A program on `path`, with `.exe` on Windows.
fn find_exe(path: &OsStr, name: &str) -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    std::env::split_paths(path)
        .map(|d| d.join(format!("{name}{suffix}")))
        .find(|p| p.is_file())
}

/// The Verilator version line, which is part of the cache key.
fn verilator_version(cfg: &VerilatorConfig, path: &OsStr) -> Result<String, VerilatorError> {
    let mut cmd = Command::new(&cfg.verilator);
    cmd.arg("--version").env("PATH", path).stdin(Stdio::null());
    if let Some(r) = &cfg.root {
        cmd.env("VERILATOR_ROOT", fwd(r));
    }
    hide_window(&mut cmd);
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(VerilatorError::Build {
            step: "verilator --version",
            diagnostics: Vec::new(),
            log: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

impl VerilatorPl {
    /// Builds the design (or reuses its cached executable), starts it and completes the
    /// handshake.
    pub fn start(cfg: &VerilatorConfig) -> Result<VerilatorPl, VerilatorError> {
        if !cfg!(windows) {
            return Err(VerilatorError::Unsupported(
                "the DPI library is built for Windows only for now".to_string(),
            ));
        }
        for p in cfg.sources.iter().chain([&cfg.dpi_lib, &cfg.verilator]) {
            if !p.is_file() {
                return Err(VerilatorError::MissingFile(p.clone()));
            }
        }
        // A std build of the same crate has the same file name but exports no `rp_*`
        // functions; catch that here instead of as a link error.
        let archive = fs::read(&cfg.dpi_lib)?;
        if !archive.windows(7).any(|w| w == b"rp_open") {
            return Err(VerilatorError::Build {
                step: "DPI library check",
                diagnostics: Vec::new(),
                log: format!(
                    "{} does not export the rp_* DPI functions: it was built without \
                     `--features freestanding`. Build it with `cargo xtask dpi`.",
                    cfg.dpi_lib.display()
                ),
            });
        }
        fs::create_dir_all(&cfg.work_dir)?;
        let work = fs::canonicalize(&cfg.work_dir)?;
        // `canonicalize` yields `\\?\` paths, which the tools do not take.
        let work = PathBuf::from(work.display().to_string().trim_start_matches(r"\\?\"));

        let path = build_path(cfg)?;
        for (tool, hint) in [
            (
                "make",
                "GNU make: Git for Windows does not ship one; MSYS2 `pacman -S make`, \
                 or a copy of MinGW's mingw32-make.exe named make.exe on the tool path",
            ),
            (
                "g++",
                "a C++ compiler: MinGW-w64 g++ (MSYS2, Qt Tools, WinLibs) on the PATH",
            ),
            (
                "sh",
                "a POSIX shell: Git for Windows (`C:\\Program Files\\Git\\usr\\bin`) on the PATH",
            ),
        ] {
            if find_exe(&path, tool).is_none() {
                return Err(VerilatorError::MissingTool { tool, hint });
            }
        }

        // The DPI archive is linked as `-LDFLAGS <file>`: a copy in the run directory, with
        // a name without spaces.
        let dpi_dir = work.join("dpi");
        fs::create_dir_all(&dpi_dir)?;
        let dpi_copy = dpi_dir.join(DPI_COPY);
        fs::copy(&cfg.dpi_lib, &dpi_copy)?;

        let version = verilator_version(cfg, &path)?;
        let key = match &cfg.cache_dir {
            Some(_) => Some(build_key(cfg, &archive, &version)?),
            None => None,
        };
        let build_log = work.join("build.log");
        let sim_exe = work.join(format!("{SIM_NAME}.exe"));
        let built = work.join("obj").join(format!("{SIM_NAME}.exe"));

        let cached = match (&cfg.cache_dir, &key) {
            (Some(dir), Some(k)) => {
                let entry = dir.join(k);
                if entry.join("ok").is_file()
                    && fs::copy(entry.join(format!("{SIM_NAME}.exe")), &sim_exe).is_ok()
                {
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
            let _ = fs::remove_dir_all(work.join("obj"));
            let mut cmd = Command::new(&cfg.verilator);
            cmd.current_dir(&work)
                .arg("--top-module")
                .arg(&cfg.top)
                .args(["-Mdir", "obj", "-o", SIM_NAME])
                .args(BUILD_FLAGS);
            for d in &cfg.defines {
                cmd.arg(format!("-D{d}"));
            }
            cmd.arg("-LDFLAGS").arg(fwd(&dpi_copy));
            for s in &cfg.sources {
                cmd.arg(fwd(s));
            }
            cmd.env("PATH", &path).stdin(Stdio::null());
            if let Some(r) = &cfg.root {
                cmd.env("VERILATOR_ROOT", fwd(r));
            }
            let out = fs::File::create(&build_log)?;
            cmd.stdout(out.try_clone()?).stderr(out);
            hide_window(&mut cmd);
            let status = cmd.status()?;
            let log = fs::read_to_string(&build_log).unwrap_or_default();
            if !status.success() {
                return Err(VerilatorError::Build {
                    step: "verilator (translate, compile and link)",
                    diagnostics: diag::errors(&log),
                    log: tail(&build_log),
                });
            }
            fs::copy(&built, &sim_exe).map_err(|e| VerilatorError::Build {
                step: "verilator (no executable was produced)",
                diagnostics: diag::errors(&log),
                log: format!("{e}\n{}", tail(&build_log)),
            })?;
            if let (Some(dir), Some(k)) = (&cfg.cache_dir, &key) {
                // Best effort: a cache that cannot be written is not an error.
                let tmp = dir.join(format!("{k}.tmp{}", std::process::id()));
                let entry = dir.join(k);
                let stored = fs::create_dir_all(&tmp)
                    .and_then(|()| fs::copy(&sim_exe, tmp.join(format!("{SIM_NAME}.exe"))))
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
        let mut cmd = Command::new(&sim_exe);
        cmd.current_dir(&work)
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
                        return Err(VerilatorError::NotConnected {
                            detail: format!("simulation exited ({st}); output:\n{}", tail(&out)),
                        });
                    }
                    if start.elapsed() > cfg.connect_timeout {
                        kill_tree(&mut child);
                        return Err(VerilatorError::NotConnected {
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
        // The DPI's hello arrives first; consuming it records its capabilities.
        match rp.recv()? {
            Packet::Hello { .. } => {}
            p => {
                kill_tree(&mut child);
                return Err(VerilatorError::NotConnected {
                    detail: format!("expected hello, got {p:?}"),
                });
            }
        }

        let sources = &cfg.sources;
        Ok(VerilatorPl {
            child,
            rp,
            finished: false,
            stats: VerilatorStats::default(),
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
    pub fn stats(&self) -> VerilatorStats {
        self.stats
    }

    /// The scratch directory holding logs and products.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Verilator's warnings for the design (`%Warning`, for example an ignored `specify`
    /// block). They do not stop the build.
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

impl PlTarget for VerilatorPl {
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

impl Drop for VerilatorPl {
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
