//! Adapter that builds an RTL design with Vivado's XSim, runs it with the
//! `xsim-dpi` library loaded, and exposes it to a PS engine as a [`PlTarget`].
//!
//! ## How it fits together
//!
//! The testbench (`top`) imports the `rp_*` DPI functions. The `xsim-dpi`
//! static library implements them and connects, as a remote-port *client*, to
//! this adapter, which plays the part QEMU plays in a direct co-simulation:
//!
//! * a PS access to the PL becomes a remote-port bus request to the DPI;
//! * the DPI's PL-master accesses (DMA) and interrupts are forwarded to the
//!   [`PsPort`] the PS engine hands in;
//! * at each PS quantum a sync request makes the testbench advance PL time to
//!   the PS time before the PS continues (lock-step).
//!
//! PL timing at its own ports is RTL-exact (XSim); coupling to the PS is
//! quantum-granular.
//!
//! ## Honest limits
//!
//! Needs a Vivado install (XSim). Windows only for now: XSim links DPI as a
//! static archive there and the DPI library is built `no_std` for that.

mod export;
pub use export::{ExportPlan, plan as plan_export};
pub use xsim_export::ExportDesign;

use interop::{
    BusAccess, BusCmd, Interrupt, Packet, Session, SessionError, Sync, cap, flags, resp,
};
use pl_api::media::{DEV_AUDIO_IN, DEV_AUDIO_OUT, DEV_CODEC_REG, DEV_VIDEO_IN, DEV_VIDEO_OUT};
use pl_api::{
    AudioSource, BoardOutput, BusError, BusResult, DEV_BOARD_IN, DEV_BOARD_OUT, DEV_M_AXI_GP0,
    DEV_PIN_IN, DEV_PIN_OUT, DEV_WIRES_PS_TO_PL, MasterAccess, MediaEvent, PinEvent, PinSink,
    PlTarget, PsMasterPort, PsPort, VideoSource,
};
use std::fmt;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Name of the DPI archive as XSim wants it (`<name>.a`, library name `<name>`).
const DPI_NAME: &str = "zynqsim_dpi";

/// How to build and run a design.
#[derive(Clone, Debug)]
pub struct XsimConfig {
    /// Vivado install directory (contains `settings64.bat`).
    pub vivado_root: PathBuf,
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
    /// Record a waveform database (`xsim.wdb`); large and slows the run.
    pub waves: bool,
    /// How long to wait for the design to connect after elaboration.
    pub connect_timeout: Duration,
    /// Where compiled snapshots are kept between runs. A design with the same
    /// sources, defines, top, DPI library and Vivado is not compiled again:
    /// reloading the firmware of a project costs a copy, not a build.
    pub cache_dir: Option<PathBuf>,
}

/// Errors from the XSim adapter.
#[derive(Debug)]
pub enum XsimError {
    /// A configured file does not exist.
    MissingFile(PathBuf),
    /// The build (xvlog/xelab) failed; the log tail is included.
    Build {
        /// Which step failed.
        step: &'static str,
        /// Last lines of the build log.
        log: String,
    },
    /// An OS error.
    Io(std::io::Error),
    /// The simulator exited, or never connected.
    NotConnected {
        /// Why, with the tail of the simulator output.
        detail: String,
    },
    /// The remote-port session failed.
    Rp(SessionError),
}

impl fmt::Display for XsimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XsimError::MissingFile(p) => write!(f, "file not found: {}", p.display()),
            XsimError::Build { step, log } => write!(f, "XSim build failed at {step}:\n{log}"),
            XsimError::Io(e) => write!(f, "XSim adapter I/O error: {e}"),
            XsimError::NotConnected { detail } => {
                write!(f, "XSim did not connect to the adapter: {detail}")
            }
            XsimError::Rp(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for XsimError {}

impl From<std::io::Error> for XsimError {
    fn from(e: std::io::Error) -> Self {
        XsimError::Io(e)
    }
}

impl From<SessionError> for XsimError {
    fn from(e: SessionError) -> Self {
        XsimError::Rp(e)
    }
}

/// Counters for tests and the status view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XsimStats {
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

/// A running XSim session.
pub struct XsimPl {
    child: Child,
    rp: Session,
    finished: bool,
    stats: XsimStats,
    master_log: Vec<MasterAccess>,
    board_out: Vec<BoardOutput>,
    media: Vec<MediaEvent>,
    video_src: VideoSource,
    audio_src: AudioSource,
    /// A waveform database is being recorded: end the run cleanly so it is valid.
    waves: bool,
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
    now_ns: u64,
}

/// Whether any of `sources` mentions one of `needles`. A design that does not
/// is not offered the matching board features.
fn sources_mention(sources: &[PathBuf], needles: &[&str]) -> bool {
    sources.iter().any(|p| {
        std::fs::read_to_string(p)
            .map(|t| needles.iter().any(|n| t.contains(n)))
            .unwrap_or(false)
    })
}

/// Snapshots kept in the build cache.
const CACHE_KEEP: usize = 6;

/// Content key of a build: everything that changes the compiled snapshot.
fn build_key(
    cfg: &XsimConfig,
    dpi_archive: &[u8],
    export_fingerprint: &[u8],
) -> Result<String, std::io::Error> {
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
    feed(b"zynqsim-xsim-snapshot-v1");
    feed(cfg.vivado_root.display().to_string().as_bytes());
    feed(cfg.top.as_bytes());
    for d in &cfg.defines {
        feed(d.as_bytes());
    }
    feed(dpi_archive);
    feed(export_fingerprint);
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

/// Copies a directory tree.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target)?;
        } else {
            fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
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

/// Forward slashes: these paths are handed to xelab, which mishandles `\`.
fn fwd(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
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

impl XsimPl {
    /// Builds the design, starts XSim and completes the handshake.
    pub fn start(cfg: &XsimConfig) -> Result<XsimPl, XsimError> {
        Self::start_with(cfg, None)
    }

    /// Like [`XsimPl::start`], with the sources of a Vivado simulation export compiled first
    /// (prj-style, each file into its library, `-L` libraries, `glbl`); `cfg.sources` are the
    /// user's extra files (a testbench) and `cfg.top` the unit to elaborate.
    pub fn start_with(
        cfg: &XsimConfig,
        export: Option<&ExportDesign>,
    ) -> Result<XsimPl, XsimError> {
        let settings = cfg.vivado_root.join("settings64.bat");
        for p in cfg.sources.iter().chain([&cfg.dpi_lib, &settings]) {
            if !p.is_file() {
                return Err(XsimError::MissingFile(p.clone()));
            }
        }
        if !cfg!(windows) {
            return Err(XsimError::NotConnected {
                detail: "the XSim adapter supports Windows only for now".to_string(),
            });
        }
        // A std build of the same crate has the same file name but exports no
        // `rp_*` functions; catch that here instead of as 15 linker errors.
        let archive = fs::read(&cfg.dpi_lib)?;
        let has_exports = archive.windows(7).any(|w| w == b"rp_open");
        if !has_exports {
            return Err(XsimError::Build {
                step: "DPI library check",
                log: format!(
                    "{} does not export the rp_* DPI functions: it was built without \
                     `--features freestanding`. Build it with `cargo xtask dpi`.",
                    cfg.dpi_lib.display()
                ),
            });
        }
        fs::create_dir_all(&cfg.work_dir)?;
        let work = fs::canonicalize(&cfg.work_dir)?;
        // `canonicalize` yields `\\?\` paths, which cmd cannot cd into.
        let work = PathBuf::from(work.display().to_string().trim_start_matches(r"\\?\"));

        // xelab links `<sv_root>/<name>.a`.
        let dpi_dir = work.join("dpi");
        fs::create_dir_all(&dpi_dir)?;
        fs::copy(&cfg.dpi_lib, dpi_dir.join(format!("{DPI_NAME}.a")))?;

        // ---- build --------------------------------------------------------
        let sources: Vec<String> = cfg
            .sources
            .iter()
            .map(|p| format!("\"{}\"", fwd(p)))
            .collect();
        // Quoted: cmd treats an unquoted `=` as an argument separator.
        let defines: String = cfg.defines.iter().map(|d| format!(" -d \"{d}\"")).collect();
        let export_plan =
            export.map(|d| export::plan(d, &cfg.sources, &cfg.top, &cfg.defines, &cfg.vivado_root));
        if let Some(p) = &export_plan {
            for f in &p.inputs {
                if !f.is_file() {
                    return Err(XsimError::MissingFile(f.clone()));
                }
            }
            p.write_prj(&work)?;
        }
        let mut build_bat = format!(
            "@echo off\r\n\
             call \"{settings}\"\r\n\
             cd /d \"{work}\"\r\n\
             rmdir /s /q xsim.dir 2>nul\r\n\
             call xvlog -sv{defines} {sources} || exit /b 1\r\n\
             call xelab -debug typical -timescale 1ns/1ps -L work work.{top} \
             -sv_root \"{dpi}\" -sv_lib {DPI_NAME} -s zs_snap || exit /b 2\r\n",
            settings = settings.display(),
            work = work.display(),
            sources = sources.join(" "),
            top = cfg.top,
            dpi = fwd(&dpi_dir),
        );
        if let Some(p) = &export_plan {
            build_bat = format!(
                "@echo off\r\ncall \"{settings}\"\r\ncd /d \"{work}\"\r\n\
                 rmdir /s /q xsim.dir 2>nul\r\n{}",
                p.batch_steps(&fwd(&dpi_dir), DPI_NAME),
                settings = settings.display(),
                work = work.display(),
            );
        }
        let build_path = work.join("build.bat");
        let build_log = work.join("build.log");
        let key = match &cfg.cache_dir {
            Some(_) => {
                let fp = match &export_plan {
                    Some(p) => p.fingerprint()?,
                    None => Vec::new(),
                };
                Some(build_key(cfg, &archive, &fp)?)
            }
            None => None,
        };
        let cached = match (&cfg.cache_dir, &key) {
            (Some(dir), Some(k)) => {
                let entry = dir.join(k);
                if entry.join("ok").is_file()
                    && copy_tree(&entry.join("xsim.dir"), &work.join("xsim.dir")).is_ok()
                {
                    fs::write(
                        &build_log,
                        format!("reused the cached build {k}; nothing was compiled\n"),
                    )?;
                    // Touch so that pruning keeps recently used snapshots.
                    let _ = fs::write(entry.join("ok"), b"ok");
                    true
                } else {
                    let _ = fs::remove_dir_all(work.join("xsim.dir"));
                    false
                }
            }
            _ => false,
        };
        if !cached {
            fs::write(&build_path, build_bat)?;
            let status = run_bat(&build_path, &build_log)?;
            if !status.success() {
                let step = match status.code() {
                    Some(1) => "xvlog (compile)",
                    Some(2) => "xelab (elaborate and link the DPI library)",
                    _ => "build",
                };
                return Err(XsimError::Build {
                    step,
                    log: tail(&build_log),
                });
            }
            if let (Some(dir), Some(k)) = (&cfg.cache_dir, &key) {
                // Best effort: a cache that cannot be written is not an error.
                let tmp = dir.join(format!("{k}.tmp{}", std::process::id()));
                let entry = dir.join(k);
                let stored = copy_tree(&work.join("xsim.dir"), &tmp.join("xsim.dir"))
                    .and_then(|()| fs::write(tmp.join("ok"), b"ok"))
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

        // ---- run ------------------------------------------------------------
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let tcl = if cfg.waves {
            format!("log_wave -recursive /{}\r\nrun all\r\nquit\r\n", cfg.top)
        } else {
            "run all\r\nquit\r\n".to_string()
        };
        fs::write(work.join("run.tcl"), tcl)?;
        let run_bat = format!(
            "@echo off\r\n\
             call \"{settings}\"\r\n\
             cd /d \"{work}\"\r\n\
             set RP_SOCKET=tcp:127.0.0.1:{port}\r\n\
             xsim zs_snap -tclbatch run.tcl\r\n",
            settings = settings.display(),
            work = work.display(),
        );
        let run_path = work.join("run.bat");
        fs::write(&run_path, run_bat)?;
        let out = work.join("xsim.out.log");
        let mut cmd = Command::new("cmd");
        cmd.arg("/C")
            .arg(&run_path)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out)?)
            .stderr(fs::File::create(work.join("xsim.err.log"))?);
        hide_window(&mut cmd);
        let mut child = cmd.spawn()?;

        // ---- accept the DPI connection ---------------------------------------
        listener.set_nonblocking(true)?;
        let start = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(Some(st)) = child.try_wait() {
                        return Err(XsimError::NotConnected {
                            detail: format!("simulator exited ({st}); output:\n{}", tail(&out)),
                        });
                    }
                    if start.elapsed() > cfg.connect_timeout {
                        kill_tree(&mut child);
                        return Err(XsimError::NotConnected {
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
                return Err(XsimError::NotConnected {
                    detail: format!("expected hello, got {p:?}"),
                });
            }
        }

        let all_sources: Vec<PathBuf> = export_plan
            .iter()
            .flat_map(|p| p.inputs.iter().cloned())
            .chain(cfg.sources.iter().cloned())
            .collect();
        Ok(XsimPl {
            child,
            rp,
            finished: false,
            stats: XsimStats::default(),
            master_log: Vec::new(),
            board_out: Vec::new(),
            media: Vec::new(),
            video_src: VideoSource::color_bars(320, 240),
            audio_src: AudioSource::Silence,
            waves: cfg.waves,
            has_media: sources_mention(
                &all_sources,
                &["zs_ssm2603", "zs_hdmi", "zs_audio_", "zs_video_"],
            ),
            has_pins: sources_mention(&all_sources, &["tel_ctrl", "tel_event"]),
            has_pin_groups: sources_mention(&all_sources, &["zs_pin_"]),
            pin_events: Vec::new(),
            pin_sink: None,
            work_dir: work,
            now_ns: 0,
        })
    }

    /// Counters.
    pub fn stats(&self) -> XsimStats {
        self.stats
    }

    /// The scratch directory holding logs and products.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Stops the simulator and everything it spawned.
    pub fn kill(&mut self) {
        kill_tree(&mut self.child);
        self.finished = true;
    }

    /// Handles one packet the design sent while we wait for something else.
    /// Returns `Some(packet)` if it is the response being waited for.
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
                    leds: i.val & 0xF,
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

    /// Media traffic from the testbench's HDMI and codec models. Returns true
    /// when the packet was one of theirs.
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
                // The synchronous exchange: report the change, answer at once
                // with what the devices drive (1 = released).
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
        let (code, data, ok) = match master_port(b.dev) {
            None => (resp::ADDR_ERROR, Vec::new(), false),
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
                    Ok(d) => (resp::OK, d, true),
                    Err(BusError::Decode) => (resp::ADDR_ERROR, Vec::new(), false),
                    Err(BusError::Slave) => (resp::BUS_GENERIC_ERROR, Vec::new(), false),
                }
            }
        };
        let _ = ok;
        if !b.is_posted() {
            let rsp = b.response(code, data, b.timestamp);
            let _ = self.rp.send(&Packet::Bus(rsp));
        }
    }

    /// Sends a PS access and waits for its response, serving everything the
    /// design sends in the meantime.
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

impl PlTarget for XsimPl {
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
                    if self.serve(other, ps).is_err() {
                        // An interrupt the PS rejected; keep waiting for the sync.
                    }
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
                val: val & 0xF,
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

impl Drop for XsimPl {
    fn drop(&mut self) {
        // The testbench ends (`$finish`) when its peer closes, and only then
        // does XSim write out the waveform database: give it a moment.
        if self.waves && !self.finished {
            self.rp.shutdown();
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.kill();
    }
}

/// Runs a generated `.bat`, sending all output to `log`.
fn run_bat(bat: &Path, log: &Path) -> std::io::Result<std::process::ExitStatus> {
    let out = fs::File::create(log)?;
    let err = out.try_clone()?;
    let mut cmd = Command::new("cmd");
    cmd.arg("/C")
        .arg(bat)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    hide_window(&mut cmd);
    cmd.status()
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

/// Kills the process and its children (`cmd` -> `xsim` -> `xsimk`).
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
