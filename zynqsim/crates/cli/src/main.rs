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
        /// After the run, save the board's Ethernet frames as a `.pcap` file
        /// (needs a `[network]` section; timestamps are virtual time).
        #[arg(long)]
        pcap: Option<PathBuf>,
        /// After the run, save the codec DAC audio (newest 10 s at most) as WAV.
        #[arg(long = "audio-out")]
        audio_out: Option<PathBuf>,
        /// After the run, save a Pmod device's picture: `JA=FILE.png`.
        #[arg(long = "device-out")]
        device_out: Vec<String>,
        /// Stop the guest here (turns the debugger on): an address, an ELF
        /// symbol or a source line `main.rs:42` (needs DWARF line info).
        /// Repeatable; the stop is reported with its file and line.
        #[arg(long = "break")]
        breakpoints: Vec<String>,
        /// Set a physical quantity of a Pmod device, at the start or at a
        /// simulated time: `JA.temperature=23.5` or `5ms@JA.temperature=-12.25`.
        #[arg(long = "device-set")]
        device_set: Vec<String>,
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
        /// Create the project from a Vivado hardware handoff (.xsa or .hwh): PS7
        /// configuration, address map, interrupts, clocks, user RTL. IP without a
        /// model is listed and makes the project refuse to run.
        #[arg(long)]
        xsa: Option<PathBuf>,
        /// With --xsa: the project directory (same as the positional DIR).
        #[arg(long)]
        out: Option<PathBuf>,
        /// With --xsa: user RTL (Verilog/SystemVerilog) the handoff does not carry;
        /// repeat for further files.
        #[arg(long = "rtl")]
        rtl: Vec<PathBuf>,
        /// Create the project from a Vivado simulation export (`export_simulation` folder,
        /// or one `.prj`/`.f` list): its sources, libraries (`-L unisims_ver`) and `glbl`
        /// are compiled by XSim; a testbench is generated for the AXI4-Lite slave in it
        /// unless it brings its own. Use `--out DIR` for the project directory.
        #[arg(long = "export-sim")]
        export_sim: Option<PathBuf>,
        /// With --xsa: a Vivado file list (`.prj` from export_simulation, or `.f`)
        /// whose Verilog/SystemVerilog files are added to the RTL. On its own it is
        /// not available yet.
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
        /// With --ip: firmware ELF for core 1 (optional).
        #[arg(long)]
        elf1: Option<String>,
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
    /// Build an SD card image (MBR plus one FAT16/FAT32 volume) from a folder.
    SdImage {
        /// Folder whose files (recursively) are put on the card.
        #[arg(long)]
        dir: PathBuf,
        /// Image file to write.
        #[arg(long)]
        out: PathBuf,
        /// Card size in MiB: a power of two from 8 to 1024. Default: the
        /// smallest that holds the files.
        #[arg(long = "size-mb")]
        size_mb: Option<u32>,
        /// `auto` (FAT16 below 64 MiB, else FAT32), `16` or `32`.
        #[arg(long, default_value = "auto")]
        fat: String,
        /// Volume label (up to 11 characters of A-Z 0-9 _ - space).
        #[arg(long, default_value = "ZYNQSIM")]
        label: String,
    },
    /// Pack a file at an offset into an erased 16 MiB QSPI flash image.
    QspiImage {
        /// File to place in the flash.
        #[arg(long)]
        file: PathBuf,
        /// Byte offset of the file in the flash (123 or 0x7b).
        #[arg(long, default_value = "0")]
        offset: String,
        /// Image file to write.
        #[arg(long)]
        out: PathBuf,
        /// Flash size in MiB (the board has 16).
        #[arg(long = "size-mb", default_value_t = 16)]
        size_mb: u32,
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
            device_out,
            breakpoints,
            device_set,
            pcap,
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
                device_out,
                device_set,
                pcap,
                breakpoints,
            },
        ),
        Cmd::Test { recording } => cmd_test(json, &recording),
        Cmd::Init {
            dir,
            xsa,
            export_sim,
            out,
            rtl,
            filelist,
            ip,
            module,
            elf,
            elf1,
            base,
        } => {
            let ip = InitIp {
                ip,
                module,
                elf,
                elf1,
                base,
            };
            match export_sim {
                Some(x) => init_from_export(json, &out.unwrap_or(dir), &x, ip),
                None => cmd_init(json, &out.unwrap_or(dir), xsa, rtl, filelist, ip),
            }
        }
        Cmd::Inspect { project } => cmd_inspect(json, project.as_deref()),
        Cmd::Trace {
            recording,
            vcd,
            perfetto,
        } => cmd_trace(json, &recording, vcd, perfetto),
        Cmd::SdImage {
            dir,
            out,
            size_mb,
            fat,
            label,
        } => cmd_sd_image(json, &dir, &out, size_mb, &fat, label),
        Cmd::QspiImage {
            file,
            offset,
            out,
            size_mb,
        } => cmd_qspi_image(json, &file, &offset, &out, size_mb),
        Cmd::Mcp { args } => launch("zynqsim-mcp", &args),
        Cmd::Gui { args } => launch("zynqsim-gui", &args),
    }
}

fn cmd_sd_image(
    json: bool,
    dir: &Path,
    out: &Path,
    size_mb: Option<u32>,
    fat: &str,
    label: String,
) -> Result<u8, Failure> {
    use sim_api::storage::{FatKind, SdImageOptions};
    let fat = match fat {
        "auto" => FatKind::Auto,
        "16" => FatKind::Fat16,
        "32" => FatKind::Fat32,
        other => {
            return Err(fail(
                2,
                format!("--fat `{other}` is not valid; expected auto, 16 or 32"),
            ));
        }
    };
    let opts = SdImageOptions {
        size_mb,
        fat,
        label,
    };
    let image = sim_api::storage::build_sd_image(dir, &opts)?;
    let files = sim_api::storage::read_sd_image(&image)?;
    let kind = sim_api::storage::sd_image_fat_type(&image)?;
    write_file(out, &image)?;
    if json {
        println!(
            "{}",
            json!({
                "image": out.display().to_string(),
                "bytes": image.len(),
                "fat": kind,
                "files": files.len(),
            })
        );
    } else {
        println!(
            "wrote {} ({} MiB, {kind}, {} file(s))",
            out.display(),
            image.len() >> 20,
            files.len()
        );
    }
    Ok(0)
}

fn cmd_qspi_image(
    json: bool,
    file: &Path,
    offset: &str,
    out: &Path,
    size_mb: u32,
) -> Result<u8, Failure> {
    let offset = parse_addr(offset)?;
    let data =
        std::fs::read(file).map_err(|e| fail(2, format!("cannot read {}: {e}", file.display())))?;
    let image = sim_api::storage::build_qspi_image(&data, offset, size_mb)?;
    write_file(out, &image)?;
    if json {
        println!(
            "{}",
            json!({
                "image": out.display().to_string(),
                "bytes": image.len(),
                "offset": offset,
                "file_bytes": data.len(),
            })
        );
    } else {
        println!(
            "wrote {} ({} MiB flash, {} byte(s) at {offset:#x})",
            out.display(),
            image.len() >> 20,
            data.len()
        );
    }
    Ok(0)
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
    device_out: Vec<String>,
    device_set: Vec<String>,
    pcap: Option<PathBuf>,
    breakpoints: Vec<String>,
}

/// `JA`..`JE` (any case) to the Pmod port index 0..=4.
fn pmod_port(s: &str) -> Result<u8, Failure> {
    match s.to_ascii_uppercase().as_str() {
        "JA" => Ok(0),
        "JB" => Ok(1),
        "JC" => Ok(2),
        "JD" => Ok(3),
        "JE" => Ok(4),
        "CAM" => Ok(5),
        other => Err(fail(
            2,
            format!("`{other}` is not a Pmod port (JA..JE) or the camera connector (CAM)"),
        )),
    }
}

/// What a scripted step of a run does.
enum Step {
    Input(sim_api::Input),
    Device { port: u8, name: String, value: f64 },
}

/// `[TIME@]PORT.NAME=VALUE`.
fn parse_device_set(spec: &str) -> Result<(Ps, Step), Failure> {
    let bad = |why: &str| fail(2, format!("--device-set `{spec}`: {why}"));
    let (time, rest) = match spec.split_once('@') {
        Some((t, r)) => (
            Ps(sim_api::parse_duration(t)
                .map_err(|e| fail(2, format!("--device-set `{spec}`: {}", e.message)))?),
            r,
        ),
        None => (Ps(0), spec),
    };
    let (target, value) = rest
        .split_once('=')
        .ok_or_else(|| bad("expected [TIME@]PORT.NAME=VALUE, e.g. JA.temperature=23.5"))?;
    let (port, name) = target
        .split_once('.')
        .ok_or_else(|| bad("expected PORT.NAME, e.g. JA.temperature"))?;
    let value: f64 = value
        .trim()
        .parse()
        .map_err(|_| bad(&format!("`{value}` is not a number")))?;
    Ok((
        time,
        Step::Device {
            port: pmod_port(port)?,
            name: name.trim().to_string(),
            value,
        },
    ))
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
    breakpoints: &[String],
) -> Result<Box<dyn sim_api::Engine>, Failure> {
    let Some(file) = project else {
        if !breakpoints.is_empty() {
            return Err(fail(
                2,
                "--break needs --project (a project with the qemu PS)",
            ));
        }
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
    if !breakpoints.is_empty() {
        let d = p.debug.get_or_insert_with(Default::default);
        d.enabled = true;
        d.breakpoints.extend(breakpoints.iter().cloned());
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
    let mut engine = build_engine(&a.project, a.seed, &a.breakpoints)?;
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

    let mut steps: Vec<(Ps, Step)> = script
        .into_iter()
        .map(|(t, i)| (t, Step::Input(i)))
        .collect();
    for spec in &a.device_set {
        steps.push(parse_device_set(spec)?);
    }
    steps.sort_by_key(|(t, _)| *t);
    for (t, step) in steps {
        if t > total {
            return Err(fail(
                2,
                format!("--at event at {t} is after the end of the run ({total})"),
            ));
        }
        engine.command(Command::RunUntil { time_ps: t.0 })?;
        match step {
            Step::Input(input) => engine.command(Command::Input(input))?,
            Step::Device { port, name, value } => {
                engine.command(Command::SetDeviceValue { port, name, value })?
            }
        };
    }
    engine.command(Command::RunUntil { time_ps: total.0 })?;

    let mut no_recording = String::from("the session outgrew its journal");
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
        Err(e) => {
            no_recording = e.message;
            None
        }
        _ => None,
    };
    if let Some(p) = &a.record {
        let rec = recording.as_ref().ok_or_else(|| {
            fail(
                3,
                format!("--record is not available on this backend ({no_recording})"),
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

    for spec in &a.device_out {
        let (port, file) = spec
            .split_once('=')
            .ok_or_else(|| fail(2, format!("--device-out `{spec}`: expected JA=FILE.png")))?;
        let idx = match port.to_ascii_uppercase().as_str() {
            "JA" => 0u8,
            "JB" => 1,
            "JC" => 2,
            "JD" => 3,
            "JE" => 4,
            "CAM" => 5,
            other => {
                return Err(fail(
                    2,
                    format!("`{other}` is not a Pmod port (JA..JE) or CAM"),
                ));
            }
        };
        match engine.query(&Query::DeviceImage { port: idx }, false)? {
            Response::DeviceImage { png, .. } => write_file(Path::new(file), &png)?,
            r => return Err(fail(2, format!("unexpected response {r:?}"))),
        }
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

    if let Some(p) = &a.pcap {
        match engine.query(&Query::NetPcap, false)? {
            Response::Pcap(bytes) => write_file(p, &bytes)?,
            r => return Err(fail(2, format!("unexpected response {r:?}"))),
        }
    }
    let network = match engine.query(&Query::NetStats, false) {
        Ok(Response::NetStats(n)) => Some(n),
        _ => None,
    };

    let st = engine.status(false);
    // Where the guest is stopped, when a breakpoint ended the run.
    let debug = st.debug.as_ref().filter(|d| d.stopped).map(|d| {
        let regs = match engine.query(
            &Query::CpuRegs {
                core: d.core.unwrap_or(0),
            },
            false,
        ) {
            Ok(Response::CpuRegs(r)) => Some(r),
            _ => None,
        };
        json!({
            "stopped": true,
            "core": d.core,
            "pc": d.pc,
            "reason": d.reason,
            "pc_symbol": regs.as_ref().and_then(|r| r.pc_symbol.clone()),
            "source": regs.as_ref().and_then(|r| r.pc_source.clone()),
        })
    });
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
    // The state of every attached device: settable quantities and registers.
    let mut device_state = Vec::new();
    for d in &st.devices {
        if let Ok(Response::DeviceState {
            port,
            kind,
            values,
            registers,
            ..
        }) = engine.query(&Query::DeviceState { port: d.port }, false)
        {
            device_state.push(json!({
                "port": port, "kind": kind, "values": values, "registers": registers,
            }));
        }
    }
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
                "devices": st.devices,
                "device_state": device_state,
                "rgb": engine.board().rgb,
                "network": network,
                "storage": st.storage,
                "debug": debug,
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
        if let Some(d) = &debug {
            let src = match (d["source"]["file"].as_str(), d["source"]["line"].as_u64()) {
                (Some(f), Some(l)) => format!(" at {f}:{l}"),
                _ => String::new(),
            };
            println!(
                "stopped   : {}{src}",
                d["reason"].as_str().unwrap_or("stopped")
            );
        }
        if bus_accesses > 0 {
            println!("pl bus    : {bus_accesses} access(es)");
        }
        if let Some(s) = &st.storage {
            for (label, dev) in [("sd card   ", &s.sd), ("qspi flash", &s.qspi)] {
                if let Some(d) = dev {
                    println!(
                        "{label}: {} ({} bytes, {}, {})",
                        d.source,
                        d.bytes,
                        d.model,
                        if d.writable {
                            "writable"
                        } else {
                            "read-only: writes are discarded"
                        }
                    );
                }
            }
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
        if let Some(n) = &network {
            println!(
                "ethernet  : {} on {} via {}, tx {} frame(s) / {} B, rx {} frame(s) / {} B \
                 (arp {}, icmp {}, udp {}, tcp {}, other {})",
                n.mac,
                n.subnet,
                n.gateway,
                n.frames_tx,
                n.bytes_tx,
                n.frames_rx,
                n.bytes_rx,
                n.arp,
                n.icmp,
                n.udp,
                n.tcp,
                n.other
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
    elf1: Option<String>,
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
    let irqs = sim_api::detect_irqs(&m, &axi);
    let master = sim_api::detect_axi_master(&m).map_err(|e| fail(2, e))?;
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
        sim_api::generate_project(
            &rtl,
            "tb/cosim_top.sv",
            &elf,
            o.elf1.as_deref(),
            &m.name,
            base,
        )
        .as_bytes(),
    )?;
    let elf_exists = dir.join(&elf).is_file();
    if json {
        println!(
            "{}",
            json!({
                "created": [file.display().to_string(), tb_path.display().to_string()],
                "module": m.name,
                "interface": axi.prefix,
                "interrupts": irqs,
                "axi_master": master.as_ref().map(|x| &x.prefix),
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
        if let Some(x) = &master {
            println!(
                "master    : {}* (AXI4) -> PS memory through S_AXI_HP0",
                x.prefix
            );
        }
        for (i, n) in irqs.iter().enumerate() {
            println!("interrupt : {n} -> IRQ_F2P[{i}] (GIC ID {})", 61 + i);
        }
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

/// `init --export-sim`: a project from a Vivado simulation export.
fn init_from_export(json: bool, dir: &Path, export: &Path, o: InitIp) -> Result<u8, Failure> {
    use sim_api::exportinit::{InitOptions, plan_init};
    if !o.ip.is_empty() {
        return Err(fail(2, "--ip cannot be combined with --export-sim"));
    }
    let base = match o.base.as_deref() {
        Some(b) => Some(parse_addr(b)?),
        None => None,
    };
    let plan = plan_init(
        export,
        dir,
        &InitOptions {
            elf: o.elf,
            elf1: o.elf1,
            module: o.module,
            base,
        },
    )
    .map_err(|e| fail(2, e.message))?;
    let created = plan.write(dir).map_err(|e| fail(2, e.message))?;
    let file = dir.join("project.toml");
    let diagnostics = Project::load(&file)
        .map(|p| p.validate(dir))
        .map_err(|e| fail(2, e.message))?;
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if json {
        println!(
            "{}",
            json!({
                "created": created.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "project": file.display().to_string(),
                "top": plan.top,
                "driven_module": plan.driven_module,
                "sources": plan.design.sources.len(),
                "libraries": plan.design.link_libs,
                "warnings": plan.warnings,
                "diagnostics": diagnostics,
                "runnable": errors == 0,
            })
        );
    } else {
        println!(
            "export    : {} source file(s), libraries {}",
            plan.design.sources.len(),
            if plan.design.link_libs.is_empty() {
                "none".to_string()
            } else {
                plan.design.link_libs.join(", ")
            }
        );
        match &plan.driven_module {
            Some(m) => println!("testbench : generated for module `{m}`"),
            None => println!("testbench : the export's own, top `{}`", plan.top),
        }
        for w in &plan.warnings {
            println!("note      : {w}");
        }
        for c in &created {
            println!("created   : {}", c.display());
        }
        for dg in &diagnostics {
            println!(
                "{:9} : {}: {}",
                match dg.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                dg.field,
                dg.message
            );
        }
        if errors == 0 {
            println!(
                "run       : zynqsim run --project {} --for 20ms",
                file.display()
            );
        } else {
            println!("status    : NOT RUNNABLE until the {errors} error(s) above are resolved");
        }
    }
    Ok(0)
}

/// `init --xsa`: a project from a Vivado hardware handoff.
fn init_from_xsa(
    json: bool,
    dir: &Path,
    xsa: &Path,
    rtl: Vec<PathBuf>,
    filelist: Option<PathBuf>,
    o: InitIp,
) -> Result<u8, Failure> {
    use sim_api::xsa;
    let opts = xsa::ImportOptions {
        rtl,
        elf: o.elf,
        elf1: o.elf1,
        filelist,
    };
    let plan = xsa::plan(xsa, dir, &opts).map_err(|e| fail(2, e.message))?;
    let created = plan.write(dir).map_err(|e| fail(2, e.message))?;
    // Validate what was written, exactly as `run` will.
    let file = dir.join("project.toml");
    let diagnostics = Project::load(&file)
        .map(|p| p.validate(dir))
        .map_err(|e| fail(2, e.message))?;
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let runnable = errors == 0;
    if json {
        println!(
            "{}",
            json!({
                "created": created.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "project": file.display().to_string(),
                "report": dir.join("xsa-report.md").display().to_string(),
                "design": plan.handoff.design.name,
                "testbench_module": plan.testbench_module,
                "unmodelled": plan.unmodelled,
                "warnings": plan.warnings,
                "diagnostics": diagnostics,
                "runnable": runnable,
            })
        );
    } else {
        let d = &plan.handoff.design;
        println!(
            "design    : {} (Vivado {}, {} PL instance(s))",
            d.name,
            d.vivado_version.as_deref().unwrap_or("?"),
            d.ips.len()
        );
        if let Some(m) = &plan.testbench_module {
            println!("rtl       : testbench drives module `{m}`");
        }
        for u in &plan.unmodelled {
            println!("unmodelled: {}", u.message());
        }
        for w in &plan.warnings {
            println!("note      : {w}");
        }
        println!("created   : {}", file.display());
        println!("report    : {}", dir.join("xsa-report.md").display());
        for dg in &diagnostics {
            println!(
                "{:9} : {}: {}",
                match dg.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                dg.field,
                dg.message
            );
        }
        if runnable {
            println!(
                "run       : zynqsim run --project {} --for 20ms",
                file.display()
            );
        } else {
            println!(
                "status    : NOT RUNNABLE until the {errors} error(s) above are resolved (see xsa-report.md)"
            );
        }
    }
    Ok(0)
}

fn cmd_init(
    json: bool,
    dir: &Path,
    xsa: Option<PathBuf>,
    rtl: Vec<PathBuf>,
    filelist: Option<PathBuf>,
    ip: InitIp,
) -> Result<u8, Failure> {
    if let Some(x) = xsa {
        if !ip.ip.is_empty() {
            return Err(fail(2, "--ip cannot be combined with --xsa"));
        }
        return init_from_xsa(json, dir, &x, rtl, filelist, ip);
    }
    if !rtl.is_empty() {
        return Err(fail(
            2,
            "--rtl is only used with --xsa (use --ip FILE for a single IP)",
        ));
    }
    if !ip.ip.is_empty() {
        if filelist.is_some() {
            return Err(fail(2, "--ip cannot be combined with --filelist"));
        }
        return init_from_ip(json, dir, ip);
    }
    if filelist.is_some() {
        return Err(fail(
            3,
            "--filelist on its own is not implemented in this build (milestone P3); combine it with --xsa",
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
