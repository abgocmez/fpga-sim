//! `zynqsim`: headless runner and launcher.
//!
//! Exit codes: 0 success, 1 a check failed (replay mismatch, validation
//! errors), 2 usage or runtime error, 3 the request needs a feature that is not
//! in this build.

mod script;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use sim_api::{
    ApiError, Command, ErrorCode, Fidelity, Project, Ps, Query, Recording, Response, Severity,
    SimConfig, Simulator, detect_backends, starter_project_toml,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "zynqsim",
    version,
    about = "ZynqSim: Zynq-7000 / Zybo Z7-20 simulator"
)]
struct Cli {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

// Parsed once at start-up and consumed immediately: boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Cmd {
    /// Run a simulation, optionally with scripted inputs.
    Run {
        /// project.toml to load (defaults to the built-in Zybo Z7-20 demo).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Simulated time to run, for example 10ms or 1.5s.
        #[arg(long = "for", default_value = "1s")]
        duration: String,
        /// Override the PRNG seed.
        #[arg(long)]
        seed: Option<u64>,
        /// Scripted input `TIME=ACTION`, repeatable. Actions: `sw0:on`, `btn1:down|up|tap`,
        /// `uart1:text` (supports \r \n), `reset[:SEED]`. Example: `--at 10ms=sw0:on`.
        #[arg(long = "at")]
        at: Vec<String>,
        /// Write the session recording (JSON) here.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Write the trace as VCD here.
        #[arg(long)]
        vcd: Option<PathBuf>,
        /// Write the trace as Perfetto JSON here.
        #[arg(long)]
        perfetto: Option<PathBuf>,
        /// Write a board screenshot (PNG) here.
        #[arg(long)]
        screenshot: Option<PathBuf>,
        /// After the run, save guest memory: `ADDR,LEN,FILE` (ADDR may be 0x...).
        #[arg(long = "dump-mem")]
        dump_mem: Vec<String>,
        /// After the run, save memory as a grayscale PNG, one pixel per 32-bit
        /// word (low byte): `ADDR,WIDTHxHEIGHT,FILE.png`.
        #[arg(long = "dump-image")]
        dump_image: Vec<String>,
        /// HDMI RX picture: `bars[:WxH]`, `gradient[:WxH]` or a PNG file.
        #[arg(long = "video-in")]
        video_in: Option<String>,
        /// Codec LINE IN audio: `silence`, `tone:HZ[:AMP]` or a WAV file.
        #[arg(long = "audio-in")]
        audio_in: Option<String>,
        /// After the run, save the newest complete HDMI TX frame as PNG.
        #[arg(long = "video-out")]
        video_out: Option<PathBuf>,
        /// After the run, save the codec DAC audio (newest 10 s at most) as WAV.
        #[arg(long = "audio-out")]
        audio_out: Option<PathBuf>,
    },
    /// Replay a recording and check that the trace digest matches.
    Test {
        /// Recording JSON written by `run --record`.
        recording: PathBuf,
    },
    /// Create a starter project.toml.
    Init {
        /// Directory to create the project in.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Import a Vivado XSA (not available yet).
        #[arg(long)]
        xsa: Option<PathBuf>,
        /// Import a Vivado simulation file list (not available yet).
        #[arg(long)]
        filelist: Option<PathBuf>,
        /// Generate a testbench and project for the AXI4-Lite slave IP in this
        /// Verilog/SystemVerilog file; repeat for further source files.
        #[arg(long = "ip")]
        ip: Vec<PathBuf>,
        /// With --ip: the module to test (default: the first module found).
        #[arg(long)]
        module: Option<String>,
        /// With --ip: firmware ELF the project runs (default fw/app.elf).
        #[arg(long)]
        elf: Option<String>,
        /// With --ip: the IP's base address, for the comment in project.toml.
        #[arg(long)]
        base: Option<String>,
    },
    /// Show the board, the backends and, with --project, validate a project.
    Inspect {
        /// Validate this project.toml and list every finding.
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Replay a recording and export its trace.
    Trace {
        /// Recording JSON.
        recording: PathBuf,
        /// Write VCD here.
        #[arg(long)]
        vcd: Option<PathBuf>,
        /// Write Perfetto JSON here.
        #[arg(long)]
        perfetto: Option<PathBuf>,
    },
    /// Start the MCP server on stdio (arguments are passed to zynqsim-mcp).
    Mcp {
        /// Arguments for zynqsim-mcp.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Open the GUI (arguments are passed to zynqsim-gui).
    Gui {
        /// Arguments for zynqsim-gui.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// A failure with the exit code it maps to.
#[derive(Debug)]
struct Failure {
    code: u8,
    message: String,
}

impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Failure {
        let code = match e.code {
            ErrorCode::Unsupported => 3,
            _ => 2,
        };
        let message = match &e.hint {
            Some(h) => format!("{} (hint: {h})", e.message),
            None => e.message,
        };
        Failure { code, message }
    }
}

fn fail(code: u8, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match dispatch(cli) {
        Ok(code) => ExitCode::from(code),
        Err(f) => {
            if json {
                println!(
                    "{}",
                    json!({ "error": { "message": f.message, "exit_code": f.code } })
                );
            } else {
                eprintln!("zynqsim: {}", f.message);
            }
            ExitCode::from(f.code)
        }
    }
}

fn dispatch(cli: Cli) -> Result<u8, Failure> {
    let json = cli.json;
    match cli.command {
        Cmd::Run {
            project,
            duration,
            seed,
            at,
            record,
            vcd,
            perfetto,
            screenshot,
            dump_mem,
            dump_image,
            video_in,
            audio_in,
            video_out,
            audio_out,
        } => cmd_run(
            json,
            RunArgs {
                project,
                duration,
                seed,
                at,
                record,
                vcd,
                perfetto,
                screenshot,
                dump_mem,
                dump_image,
                video_in,
                audio_in,
                video_out,
                audio_out,
            },
        ),
        Cmd::Test { recording } => cmd_test(json, &recording),
        Cmd::Init {
            dir,
            xsa,
            filelist,
            ip,
            module,
            elf,
            base,
        } => cmd_init(
            json,
            &dir,
            xsa,
            filelist,
            InitIp {
                ip,
                module,
                elf,
                base,
            },
        ),
        Cmd::Inspect { project } => cmd_inspect(json, project.as_deref()),
        Cmd::Trace {
            recording,
            vcd,
            perfetto,
        } => cmd_trace(json, &recording, vcd, perfetto),
        Cmd::Mcp { args } => launch("zynqsim-mcp", &args),
        Cmd::Gui { args } => launch("zynqsim-gui", &args),
    }
}

struct RunArgs {
    project: Option<PathBuf>,
    duration: String,
    seed: Option<u64>,
    at: Vec<String>,
    record: Option<PathBuf>,
    vcd: Option<PathBuf>,
    perfetto: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    dump_mem: Vec<String>,
    dump_image: Vec<String>,
    video_in: Option<String>,
    audio_in: Option<String>,
    video_out: Option<PathBuf>,
    audio_out: Option<PathBuf>,
}

fn parse_addr(s: &str) -> Result<u64, Failure> {
    let t = s.trim();
    let r = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16),
        None => t.parse::<u64>(),
    };
    r.map_err(|_| fail(2, format!("`{s}` is not an address (use 123 or 0x7b)")))
}

/// Reads `len` bytes of guest memory in 1 MiB pieces.
fn read_guest(engine: &mut dyn sim_api::Engine, addr: u64, len: u64) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::with_capacity(len as usize);
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(1 << 20) as u32;
        match engine.query(
            &Query::MemRead {
                addr: addr + done,
                len: n,
            },
            false,
        )? {
            Response::Memory { data, .. } => out.extend_from_slice(&data),
            r => return Err(fail(2, format!("unexpected response {r:?}"))),
        }
        done += u64::from(n);
    }
    Ok(out)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes)
        .map_err(|e| fail(2, format!("cannot write {}: {e}", path.display())))
}

/// Builds the engine a run asks for: the project's, or the built-in demo.
fn build_engine(
    project: &Option<PathBuf>,
    seed: Option<u64>,
) -> Result<Box<dyn sim_api::Engine>, Failure> {
    let Some(file) = project else {
        let mut cfg = SimConfig::default();
        if let Some(s) = seed {
            cfg.seed = s;
        }
        return Ok(Box::new(Simulator::new(cfg)?));
    };
    let mut p = Project::load(file)?;
    if let Some(s) = seed {
        p.sim.seed = s;
    }
    let base = file.parent().unwrap_or(Path::new("."));
    let errors: Vec<String> = p
        .validate(base)
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.field, d.message))
        .collect();
    if !errors.is_empty() {
        // A backend that is not built is "unsupported", everything else is a
        // plain project error.
        let unsupported = errors
            .iter()
            .any(|e| e.contains("not implemented in this build"));
        return Err(fail(
            if unsupported { 3 } else { 1 },
            format!(
                "project has {} error(s):\n  {}",
                errors.len(),
                errors.join("\n  ")
            ),
        ));
    }
    Ok(p.build_engine(base)?)
}

fn cmd_run(json: bool, a: RunArgs) -> Result<u8, Failure> {
    let total = Ps(sim_api::parse_duration(&a.duration)?);
    let mut engine = build_engine(&a.project, a.seed)?;
    let script = script::parse(&a.at, engine.def())?;

    if a.video_in.is_some() || a.audio_in.is_some() {
        let cwd = Path::new(".");
        let video = a
            .video_in
            .as_deref()
            .map(|v| sim_api::parse_video_spec(v, cwd))
            .transpose()?;
        let audio = a
            .audio_in
            .as_deref()
            .map(|v| sim_api::parse_audio_spec(v, cwd))
            .transpose()?;
        engine.command(Command::SetMediaSource { video, audio })?;
    }

    for (t, input) in script {
        if t > total {
            return Err(fail(
                2,
                format!("--at event at {t} is after the end of the run ({total})"),
            ));
        }
        engine.command(Command::RunUntil { time_ps: t.0 })?;
        engine.command(Command::Input(input))?;
    }
    engine.command(Command::RunUntil { time_ps: total.0 })?;

    let recording = match engine.query(&Query::Recording, false) {
        Ok(Response::Recording(mut r)) => {
            // A session of a real engine can only be replayed with the project
            // it was built from.
            if !r.journal.is_empty() {
                r.project = a
                    .project
                    .as_ref()
                    .and_then(|p| std::path::absolute(p).ok())
                    .map(|p| p.to_string_lossy().into_owned());
            }
            Some(*r)
        }
        _ => None,
    };
    if let Some(p) = &a.record {
        let rec = recording.as_ref().ok_or_else(|| {
            fail(
                3,
                "--record is not available on this backend (the session outgrew its journal)",
            )
        })?;
        let text = serde_json::to_string_pretty(rec).map_err(|e| fail(2, e.to_string()))?;
        write_file(p, text.as_bytes())?;
    }
    if let Some(p) = &a.vcd {
        write_file(p, sim_api::trace::to_vcd(engine.trace()).as_bytes())?;
    }
    if let Some(p) = &a.perfetto {
        write_file(
            p,
            sim_api::trace::to_perfetto_json(engine.trace()).as_bytes(),
        )?;
    }
    if let Some(p) = &a.screenshot {
        let png = sim_api::board::render_png(engine.def(), engine.board(), 1.0)
            .map_err(|e| fail(2, e))?;
        write_file(p, &png)?;
    }

    for spec in &a.dump_mem {
        let parts: Vec<&str> = spec.splitn(3, ',').collect();
        if parts.len() != 3 {
            return Err(fail(
                2,
                format!("--dump-mem `{spec}`: expected ADDR,LEN,FILE"),
            ));
        }
        let (addr, len) = (parse_addr(parts[0])?, parse_addr(parts[1])?);
        let data = read_guest(engine.as_mut(), addr, len)?;
        write_file(Path::new(parts[2]), &data)?;
    }
    for spec in &a.dump_image {
        let parts: Vec<&str> = spec.splitn(3, ',').collect();
        let dims = parts.get(1).and_then(|d| d.split_once('x'));
        let (Some((w, h)), 3) = (dims, parts.len()) else {
            return Err(fail(
                2,
                format!("--dump-image `{spec}`: expected ADDR,WIDTHxHEIGHT,FILE.png"),
            ));
        };
        let addr = parse_addr(parts[0])?;
        let (w, h): (u32, u32) = (
            w.trim()
                .parse()
                .map_err(|_| fail(2, format!("bad width in `{spec}`")))?,
            h.trim()
                .parse()
                .map_err(|_| fail(2, format!("bad height in `{spec}`")))?,
        );
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 1 << 22 {
            return Err(fail(2, format!("--dump-image `{spec}`: size out of range")));
        }
        let raw = read_guest(engine.as_mut(), addr, u64::from(w) * u64::from(h) * 4)?;
        // One pixel per 32-bit little-endian word; the low byte is the value.
        let rgba: Vec<u8> = raw
            .chunks_exact(4)
            .flat_map(|px| [px[0], px[0], px[0], 255])
            .collect();
        let png = sim_api::board::encode_rgba_png(w, h, rgba).map_err(|e| fail(2, e))?;
        write_file(Path::new(parts[2]), &png)?;
    }

    if let Some(p) = &a.video_out {
        match engine.query(&Query::VideoFrame, false)? {
            Response::Video { png, .. } => write_file(p, &png)?,
            r => return Err(fail(2, format!("unexpected response {r:?}"))),
        }
    }
    if let Some(p) = &a.audio_out {
        match engine.query(&Query::AudioCapture { max_frames: 0 }, false)? {
            Response::Audio { wav, .. } => write_file(p, &wav)?,
            r => return Err(fail(2, format!("unexpected response {r:?}"))),
        }
    }

    let st = engine.status(false);
    let mut uart = |port: u8| -> String {
        match engine.query(
            &Query::UartRead {
                port,
                from: 0,
                max: 1 << 20,
            },
            false,
        ) {
            Ok(Response::Uart { data, .. }) => String::from_utf8_lossy(&data).into_owned(),
            _ => String::new(),
        }
    };
    let (u0, u1) = (uart(0), uart(1));
    let leds: Vec<Value> = engine
        .def()
        .leds
        .iter()
        .zip(&engine.board().leds)
        .map(|(l, d)| json!({ "id": l.id, "duty": d }))
        .collect();
    let bus_accesses = engine
        .trace()
        .records()
        .iter()
        .filter(|r| matches!(r.event, sim_api::trace::TraceEvent::Bus { .. }))
        .count();

    if json {
        println!(
            "{}",
            json!({
                "backend": { "ps": st.ps_backend, "pl": st.pl_backend, "fidelity": st.fidelity },
                "board": st.board,
                "seed": st.seed,
                "now_ps": st.now_ps,
                "trace_len": st.trace_len,
                "trace_hash": st.trace_hash,
                "bus_accesses": bus_accesses,
                "trace_dropped": engine.trace().dropped(),
                "media": st.media,
                "leds": leds,
                "uart": { "0": u0, "1": u1 },
                "inputs": recording.as_ref().map(|r| r.entries.len()),
            })
        );
    } else {
        let note = match st.fidelity {
            Fidelity::Demo => "fidelity: demo, no CPU or RTL is simulated".to_string(),
            f => format!("fidelity: {f:?} (functional, instruction-count timing)"),
        };
        println!(
            "zynqsim run: board={} ps={} pl={} {note}",
            st.board, st.ps_backend, st.pl_backend
        );
        println!("time      : {}", Ps(st.now_ps));
        println!("trace     : {} records, {}", st.trace_len, st.trace_hash);
        if bus_accesses > 0 {
            println!("pl bus    : {bus_accesses} access(es)");
        }
        if let Some(m) = &st.media {
            println!(
                "hdmi tx   : {} frame(s){}",
                m.video_frames,
                m.video_size
                    .map(|(w, h)| format!(", {w}x{h}"))
                    .unwrap_or_default()
            );
            println!(
                "audio out : {} frame(s), peak {}, {} codec register write(s)",
                m.audio_frames, m.audio_peak, m.codec_writes
            );
        }
        let on: Vec<String> = engine
            .def()
            .leds
            .iter()
            .zip(&engine.board().leds)
            .map(|(l, d)| format!("{}={}", l.id, if *d >= 128 { "on" } else { "off" }))
            .collect();
        println!("leds      : {}", on.join(" "));
        println!("uart1 tx  : {u1:?}");
        if !u0.is_empty() {
            println!("uart0 tx  : {u0:?}");
        }
    }
    Ok(0)
}

fn read_recording(path: &Path) -> Result<Recording, Failure> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| fail(2, format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| fail(2, format!("{} is not a recording: {e}", path.display())))
}

fn cmd_test(json: bool, path: &Path) -> Result<u8, Failure> {
    let rec = read_recording(path)?;
    let report = if rec.journal.is_empty() {
        Simulator::replay(&rec)?.1
    } else {
        sim_api::replay_recording(&rec)?
    };
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else if report.matches {
        println!(
            "PASS: replay of {} step(s) to {} reproduced {}",
            rec.entries.len().max(rec.journal.len()),
            Ps(report.end_ps),
            report.trace_hash
        );
    } else {
        println!(
            "FAIL: replay produced {} but the recording expects {}",
            report.trace_hash, report.expected_hash
        );
    }
    Ok(if report.matches { 0 } else { 1 })
}

fn cmd_trace(
    json: bool,
    path: &Path,
    vcd: Option<PathBuf>,
    perfetto: Option<PathBuf>,
) -> Result<u8, Failure> {
    if vcd.is_none() && perfetto.is_none() {
        return Err(fail(2, "give --vcd and/or --perfetto"));
    }
    let rec = read_recording(path)?;
    let (trace, report) = if rec.journal.is_empty() {
        let (sim, report) = Simulator::replay(&rec)?;
        (sim.trace().clone(), report)
    } else {
        let (engine, report) = sim_api::replay_engine(&rec)?;
        (engine.trace().clone(), report)
    };
    if let Some(p) = &vcd {
        write_file(p, sim_api::trace::to_vcd(&trace).as_bytes())?;
    }
    if let Some(p) = &perfetto {
        write_file(p, sim_api::trace::to_perfetto_json(&trace).as_bytes())?;
    }
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        println!(
            "exported {} record(s); replay {}",
            report.trace_len,
            if report.matches {
                "matches the recording"
            } else {
                "DIFFERS from the recording"
            }
        );
    }
    Ok(if report.matches { 0 } else { 1 })
}

/// The `--ip` options of `init`.
struct InitIp {
    ip: Vec<PathBuf>,
    module: Option<String>,
    elf: Option<String>,
    base: Option<String>,
}

/// `init --ip`: a testbench and project for an AXI4-Lite slave IP.
fn init_from_ip(json: bool, dir: &Path, o: InitIp) -> Result<u8, Failure> {
    let mut found = None;
    let mut last_err = String::new();
    for f in &o.ip {
        let src = std::fs::read_to_string(f)
            .map_err(|e| fail(2, format!("cannot read {}: {e}", f.display())))?;
        match sim_api::parse_module(&src, o.module.as_deref()) {
            Ok(m) => {
                found = Some(m);
                break;
            }
            Err(e) => last_err = format!("{}: {e}", f.display()),
        }
    }
    let m = found.ok_or_else(|| fail(2, last_err))?;
    let axi = sim_api::detect_axi_lite(&m).map_err(|e| fail(2, e))?;
    let base = match o.base.as_deref() {
        Some(b) => parse_addr(b)?,
        None => 0x43C0_0000,
    };
    let file = dir.join("project.toml");
    let tb_path = dir.join("tb").join("cosim_top.sv");
    for p in [&file, &tb_path] {
        if p.exists() {
            return Err(fail(
                2,
                format!("{} already exists; refusing to overwrite", p.display()),
            ));
        }
    }
    std::fs::create_dir_all(dir.join("tb"))
        .map_err(|e| fail(2, format!("cannot create {}: {e}", dir.display())))?;
    write_file(&tb_path, sim_api::generate_testbench(&m, &axi).as_bytes())?;
    let rtl: Vec<String> =
        o.ip.iter()
            .map(|p| sim_api::relative_path(dir, p))
            .collect();
    let elf = o.elf.unwrap_or_else(|| "fw/app.elf".to_string());
    write_file(
        &file,
        sim_api::generate_project(&rtl, "tb/cosim_top.sv", &elf, &m.name, base).as_bytes(),
    )?;
    let elf_exists = dir.join(&elf).is_file();
    if json {
        println!(
            "{}",
            json!({
                "created": [file.display().to_string(), tb_path.display().to_string()],
                "module": m.name,
                "interface": axi.prefix,
                "addr_width": axi.addr_width,
                "clock": axi.clock,
                "reset": axi.reset.as_ref().map(|r| &r.0),
                "elf": elf,
                "elf_exists": elf_exists,
            })
        );
    } else {
        println!(
            "module    : {} (AXI4-Lite {}*, {} address bits)",
            m.name, axi.prefix, axi.addr_width
        );
        println!(
            "clock/rst : {} / {}",
            axi.clock,
            axi.reset.as_ref().map_or("none".to_string(), |r| format!(
                "{} (active {})",
                r.0,
                if r.1 { "low" } else { "high" }
            ))
        );
        println!("created   : {}", file.display());
        println!("created   : {}", tb_path.display());
        if !elf_exists {
            println!(
                "next      : build your firmware to {} (the project does not validate until it exists)",
                dir.join(&elf).display()
            );
        }
        println!(
            "run       : zynqsim run --project {} --for 20ms",
            file.display()
        );
    }
    Ok(0)
}

fn cmd_init(
    json: bool,
    dir: &Path,
    xsa: Option<PathBuf>,
    filelist: Option<PathBuf>,
    ip: InitIp,
) -> Result<u8, Failure> {
    if !ip.ip.is_empty() {
        if xsa.is_some() || filelist.is_some() {
            return Err(fail(2, "--ip cannot be combined with --xsa or --filelist"));
        }
        return init_from_ip(json, dir, ip);
    }
    if xsa.is_some() || filelist.is_some() {
        return Err(fail(
            3,
            "--xsa and --filelist ingestion is not implemented in this build (milestone P3)",
        ));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| fail(2, format!("cannot create {}: {e}", dir.display())))?;
    let file = dir.join("project.toml");
    if file.exists() {
        return Err(fail(
            2,
            format!("{} already exists; refusing to overwrite", file.display()),
        ));
    }
    write_file(&file, starter_project_toml().as_bytes())?;
    if json {
        println!("{}", json!({ "created": file.display().to_string() }));
    } else {
        println!("created {}", file.display());
    }
    Ok(0)
}

fn cmd_inspect(json: bool, project: Option<&Path>) -> Result<u8, Failure> {
    let def = sim_api::board::BoardDef::zybo_z7_20();
    let backends = detect_backends();
    let mut diagnostics = Vec::new();
    let mut errors = 0;
    if let Some(file) = project {
        let p = Project::load(file)?;
        diagnostics = p.validate(file.parent().unwrap_or(Path::new(".")));
        errors = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
    }

    if json {
        println!(
            "{}",
            json!({
                "board": {
                    "name": def.name, "part": def.part, "ddr_mb": def.ddr_mb,
                    "pin_status": def.pin_status,
                    "switches": def.switches.len(), "buttons": def.buttons.len(),
                    "leds": def.leds.len(), "rgb_leds": def.rgb_leds.len(),
                    "pmods": def.pmods.len(),
                },
                "fidelity_of_implemented_backends": Fidelity::Demo,
                "backends": backends,
                "diagnostics": diagnostics,
            })
        );
    } else {
        println!(
            "board   : {} ({}), {} MiB DDR, pins {}",
            def.display_name, def.part, def.ddr_mb, def.pin_status
        );
        println!(
            "controls: {} switches, {} buttons, {} LEDs, {} RGB LEDs, {} Pmods",
            def.switches.len(),
            def.buttons.len(),
            def.leds.len(),
            def.rgb_leds.len(),
            def.pmods.len()
        );
        println!("backends:");
        for b in &backends {
            let state = if b.implemented {
                "implemented".to_string()
            } else {
                match b.found {
                    Some(true) => "not implemented (tool found)".to_string(),
                    Some(false) => "not implemented (tool not found)".to_string(),
                    None => "not implemented".to_string(),
                }
            };
            println!(
                "  {:<10} {:?}/{:?}: {state}; {}",
                b.name, b.role, b.kind, b.note
            );
        }
        for d in &diagnostics {
            println!("{:?}: {}: {}", d.severity, d.field, d.message);
        }
    }
    Ok(if errors > 0 { 1 } else { 0 })
}

/// Runs a sibling binary, forwarding arguments and the exit code.
fn launch(name: &str, args: &[String]) -> Result<u8, Failure> {
    let exe_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&exe_name)))
        .filter(|p| p.is_file());
    let program = sibling
        .or_else(|| sim_api::find_on_path(name))
        .ok_or_else(|| {
            fail(
                2,
                format!("cannot find `{name}` next to zynqsim or on PATH; build it with `cargo build --release`"),
            )
        })?;
    let status = std::process::Command::new(&program)
        .args(args)
        .status()
        .map_err(|e| fail(2, format!("cannot start {}: {e}", program.display())))?;
    Ok(status.code().map_or(2, |c| c.clamp(0, 255) as u8))
}
