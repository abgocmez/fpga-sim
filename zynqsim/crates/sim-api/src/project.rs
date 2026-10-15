use crate::error::ApiError;
use crate::recording::SimConfig;
use board_zybo::BoardDef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `[board]` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardSection {
    /// Built-in board machine name.
    pub name: String,
    /// DDR size in MiB; informational until a PS backend consumes it.
    #[serde(default)]
    pub ddr_mb: Option<u32>,
}

/// `[backends]` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendsSection {
    /// PS backend: `demo`, `qemu` or `native`.
    #[serde(default = "demo")]
    pub ps: String,
    /// PL backend: `demo`, `verilator`, `icarus`, `xsim` or `native`.
    #[serde(default = "demo")]
    pub pl: String,
}

fn demo() -> String {
    "demo".to_string()
}

impl Default for BackendsSection {
    fn default() -> Self {
        BackendsSection {
            ps: demo(),
            pl: demo(),
        }
    }
}

/// `[sim]` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSection {
    /// Lock-step quantum in nanoseconds.
    #[serde(default = "default_quantum_ns")]
    pub quantum_ns: u64,
    /// PRNG seed.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// QEMU `-icount shift=` value (used by the QEMU backend).
    #[serde(default = "default_icount_shift")]
    pub icount_shift: u32,
}

fn default_quantum_ns() -> u64 {
    1000
}
fn default_seed() -> u64 {
    1
}
fn default_icount_shift() -> u32 {
    1
}

impl Default for SimSection {
    fn default() -> Self {
        SimSection {
            quantum_ns: default_quantum_ns(),
            seed: default_seed(),
            icount_shift: default_icount_shift(),
        }
    }
}

/// `[boot]` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootSection {
    /// `elf` or `boot_bin`.
    pub mode: String,
}

/// `[cpu0]` / `[cpu1]` sections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuSection {
    /// Path to the ELF, relative to the project file.
    pub elf: String,
}

/// `[rtl]` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtlSection {
    /// Source file patterns, relative to the project file.
    #[serde(default)]
    pub files: Vec<String>,
    /// Top module name.
    #[serde(default)]
    pub top: Option<String>,
    /// Preprocessor defines.
    #[serde(default)]
    pub defines: Vec<String>,
    /// Record a waveform database in the run directory.
    #[serde(default)]
    pub waves: bool,
}

/// `[media]` section: what the board's HDMI RX and audio LINE IN receive.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaSection {
    /// `bars[:WxH]`, `gradient[:WxH]` or a PNG path.
    #[serde(default)]
    pub video_in: Option<String>,
    /// `silence`, `tone:HZ[:AMPLITUDE]` or a WAV path.
    #[serde(default)]
    pub audio_in: Option<String>,
}

/// `[debug]` section: run the CPUs under QEMU's gdb stub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugSection {
    /// Attach the debugger. Breakpoints, watchpoints and `cpu_regs` then work,
    /// but timing is no longer reproducible between runs.
    #[serde(default)]
    pub enabled: bool,
    /// Execution breakpoints planted before the first instruction: addresses
    /// (`0x100614`) or ELF symbols (`main`).
    #[serde(default)]
    pub breakpoints: Vec<String>,
}

/// A `project.toml` (prompt §12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Schema version; currently 1.
    pub schema_version: u32,
    /// Board selection.
    pub board: BoardSection,
    /// Backend selection.
    #[serde(default)]
    pub backends: BackendsSection,
    /// Simulation settings.
    #[serde(default)]
    pub sim: SimSection,
    /// Boot mode.
    #[serde(default)]
    pub boot: Option<BootSection>,
    /// Software for core 0.
    #[serde(default)]
    pub cpu0: Option<CpuSection>,
    /// Software for core 1.
    #[serde(default)]
    pub cpu1: Option<CpuSection>,
    /// PL sources.
    #[serde(default)]
    pub rtl: Option<RtlSection>,
    /// Connection table: instance port to PS port or board pin.
    #[serde(default)]
    pub connect: BTreeMap<String, String>,
    /// Board devices attached to connectors.
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
    /// Host-side bindings (UART, network, audio).
    #[serde(default)]
    pub host: BTreeMap<String, String>,
    /// Native PL models: instance name to model name (`u_math = "my_math_ip"`).
    #[serde(default)]
    pub native: BTreeMap<String, String>,
    /// HDMI RX and audio LINE IN sources.
    #[serde(default)]
    pub media: Option<MediaSection>,
    /// Debugger.
    #[serde(default)]
    pub debug: Option<DebugSection>,
}

/// Parses a video source description (see [`MediaSection::video_in`]).
/// `base` resolves relative file names.
pub fn parse_video_spec(text: &str, base: &Path) -> Result<crate::VideoSpec, ApiError> {
    let t = text.trim();
    let size = |rest: Option<&str>| -> Result<(u32, u32), ApiError> {
        match rest {
            None => Ok((320, 240)),
            Some(r) => {
                let (w, h) = r.split_once(['x', 'X']).ok_or_else(|| {
                    ApiError::invalid_argument(format!("video size `{r}` is not WxH"))
                })?;
                let num = |v: &str| {
                    v.trim().parse::<u32>().map_err(|_| {
                        ApiError::invalid_argument(format!("video size `{r}` is not WxH"))
                    })
                };
                Ok((num(w)?, num(h)?))
            }
        }
    };
    let (kind, rest) = match t.split_once(':') {
        Some((k, r)) => (k, Some(r)),
        None => (t, None),
    };
    match kind {
        "bars" | "colorbars" => {
            let (width, height) = size(rest)?;
            Ok(crate::VideoSpec::ColorBars { width, height })
        }
        "gradient" => {
            let (width, height) = size(rest)?;
            Ok(crate::VideoSpec::Gradient { width, height })
        }
        _ => {
            let path = resolve(base, t);
            let bytes = std::fs::read(&path).map_err(|e| {
                ApiError::io(format!(
                    "video_in `{t}` is not `bars`, `gradient` or a readable PNG: {e}"
                ))
            })?;
            let (width, height, rgb) =
                board_zybo::decode_png_rgb(&bytes).map_err(ApiError::invalid_argument)?;
            Ok(crate::VideoSpec::Image { width, height, rgb })
        }
    }
}

/// Parses an audio source description (see [`MediaSection::audio_in`]).
pub fn parse_audio_spec(text: &str, base: &Path) -> Result<crate::AudioSpec, ApiError> {
    let t = text.trim();
    if t == "silence" {
        return Ok(crate::AudioSpec::Silence);
    }
    if let Some(rest) = t.strip_prefix("tone:") {
        let mut it = rest.split(':');
        let num = |v: &str| {
            v.trim().parse::<f64>().map_err(|_| {
                ApiError::invalid_argument(format!("audio_in `{t}`: `{v}` is not a number"))
            })
        };
        let freq_hz = num(it.next().unwrap_or(""))?;
        let amplitude = match it.next() {
            Some(a) => num(a)?,
            None => 0.5,
        };
        if !(20.0..=20_000.0).contains(&freq_hz) || !(0.0..=1.0).contains(&amplitude) {
            return Err(ApiError::invalid_argument(format!(
                "audio_in `{t}`: frequency must be 20..20000 Hz and amplitude 0..1"
            )));
        }
        return Ok(crate::AudioSpec::Tone { freq_hz, amplitude });
    }
    let path = resolve(base, t);
    let bytes = std::fs::read(&path).map_err(|e| {
        ApiError::io(format!(
            "audio_in `{t}` is not `silence`, `tone:HZ[:AMP]` or a readable WAV: {e}"
        ))
    })?;
    let (samples, rate) = pl_api::media::parse_wav(&bytes).map_err(ApiError::invalid_argument)?;
    Ok(crate::AudioSpec::Pcm {
        samples: pl_api::media::resample_stereo(&samples, rate),
    })
}

/// Severity of a validation finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The project cannot run as written.
    Error,
    /// The project runs but something is probably unintended.
    Warning,
}

/// One validation finding, anchored to a field path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Error or warning.
    pub severity: Severity,
    /// Dotted field path, for example `connect."u_accel.s_axi"`.
    pub field: String,
    /// What is wrong and what is accepted.
    pub message: String,
}

fn err(field: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        field: field.to_string(),
        message: message.into(),
    }
}

fn warn(field: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        field: field.to_string(),
        message: message.into(),
    }
}

const PS_BACKENDS: &[&str] = &["demo", "qemu", "native"];
const PL_BACKENDS: &[&str] = &["demo", "verilator", "icarus", "xsim", "native"];
/// PS backends that exist in this build.
const IMPLEMENTED_PS: &[&str] = &["demo", "qemu"];
/// PL backends that exist in this build.
const IMPLEMENTED_PL: &[&str] = &["demo", "native", "xsim"];

impl Project {
    /// Parses project text.
    pub fn from_toml(text: &str) -> Result<Project, ApiError> {
        toml::from_str(text).map_err(|e| ApiError::invalid_argument(format!("project.toml: {e}")))
    }

    /// The project as `project.toml` text.
    pub fn to_toml(&self) -> Result<String, ApiError> {
        toml::to_string_pretty(self)
            .map_err(|e| ApiError::internal(format!("cannot write project.toml: {e}")))
    }

    /// Reads and parses a project file.
    pub fn load(path: &Path) -> Result<Project, ApiError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
        Project::from_toml(&text)
    }

    /// The `(gp, base address)` a native instance is connected to, from the
    /// `[connect]` entry `"<instance>.<port>" = "M_AXI_GP<n>@<addr>"`.
    pub fn native_placement(&self, instance: &str) -> Option<(u8, u64)> {
        let prefix = format!("{instance}.");
        self.connect.iter().find_map(|(k, v)| {
            if !k.starts_with(&prefix) {
                return None;
            }
            let (port, addr) = v.split_once('@')?;
            let gp = match port.trim() {
                "M_AXI_GP0" => 0,
                "M_AXI_GP1" => 1,
                _ => return None,
            };
            let a = addr.trim();
            let hex = a.strip_prefix("0x").or_else(|| a.strip_prefix("0X"))?;
            Some((gp, u64::from_str_radix(hex, 16).ok()?))
        })
    }

    /// Validates the project and builds the engine it asks for.
    ///
    /// `base` is the directory relative paths resolve against. Fails with every
    /// validation error listed, or with a specific message when an external
    /// tool (QEMU, its device tree) cannot be found.
    pub fn build_engine(&self, base: &Path) -> Result<Box<dyn crate::Engine>, ApiError> {
        // External tools run in their own working directories, so every path
        // handed to them must be absolute.
        let base = &std::path::absolute(base)
            .map_err(|e| ApiError::io(format!("cannot resolve {}: {e}", base.display())))?;
        let errors: Vec<String> = self
            .validate(base)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.field, d.message))
            .collect();
        if !errors.is_empty() {
            return Err(ApiError::invalid_argument(format!(
                "project has {} error(s):\n  {}",
                errors.len(),
                errors.join("\n  ")
            )));
        }
        match self.backends.ps.as_str() {
            "demo" => Ok(Box::new(crate::Simulator::new(self.sim_config())?)),
            "qemu" => {
                let qemu = std::env::var_os("ZYNQSIM_QEMU")
                    .map(PathBuf::from)
                    .or_else(|| crate::tools::locate_binary("qemu-system-aarch64"))
                    .ok_or_else(|| {
                        ApiError::not_found("the Xilinx QEMU (qemu-system-aarch64) was not found")
                            .with_hint(
                                "install Vitis, put qemu-system-aarch64 on PATH, or set ZYNQSIM_QEMU",
                            )
                    })?;
                let dtb = std::env::var_os("ZYNQSIM_DTB")
                    .map(PathBuf::from)
                    .or_else(|| backend_qemu::QemuConfig::dtb_for(&qemu))
                    .ok_or_else(|| {
                        ApiError::not_found(format!(
                            "zynq-arm-cosim.dtb not found next to {}",
                            qemu.display()
                        ))
                        .with_hint("set ZYNQSIM_DTB to the co-simulation device tree")
                    })?;
                let cpu = |c: &Option<CpuSection>| c.as_ref().map(|c| resolve(base, &c.elf));
                let elf0 = cpu(&self.cpu0)
                    .ok_or_else(|| ApiError::invalid_argument("cpu0.elf is required"))?;
                let mut natives = Vec::new();
                for (name, model) in &self.native {
                    let (gp, addr) = self.native_placement(name).ok_or_else(|| {
                        ApiError::invalid_argument(format!("native.{name} has no [connect] entry"))
                    })?;
                    natives.push(crate::NativeInstance {
                        name: name.clone(),
                        model: model.clone(),
                        gp,
                        base: addr,
                    });
                }
                let rtl = if self.backends.pl == "xsim" {
                    let r = self.rtl.as_ref().ok_or_else(|| {
                        ApiError::invalid_argument("backends.pl = \"xsim\" needs [rtl]")
                    })?;
                    let vivado_root = std::env::var_os("ZYNQSIM_VIVADO")
                        .map(PathBuf::from)
                        .or_else(crate::tools::find_vivado)
                        .ok_or_else(|| {
                            ApiError::not_found("Vivado (XSim) was not found").with_hint(
                                "install Vivado, set XILINX_VIVADO, or set ZYNQSIM_VIVADO to the install directory",
                            )
                        })?;
                    let dpi_lib = crate::tools::find_dpi_lib().ok_or_else(|| {
                        ApiError::not_found("the XSim DPI library (libxsim_dpi.a) was not found")
                            .with_hint("build it with `cargo xtask dpi`, or set ZYNQSIM_DPI_LIB")
                    })?;
                    Some(crate::RtlSpec {
                        vivado_root,
                        sources: r.files.iter().map(|f| resolve(base, f)).collect(),
                        top: r.top.clone().unwrap_or_default(),
                        defines: r.defines.clone(),
                        dpi_lib,
                        waves: r.waves,
                    })
                } else {
                    None
                };
                let uart = match self.host.get("uart1").map(String::as_str) {
                    Some("tcp") => backend_qemu::UartMode::Tcp,
                    _ => backend_qemu::UartMode::File,
                };
                let debug = self.debug.clone().unwrap_or_default();
                let spec = crate::QemuSpec {
                    debug: debug.enabled,
                    breakpoints: debug.breakpoints.clone(),
                    qemu,
                    dtb,
                    elf0,
                    elf1: cpu(&self.cpu1),
                    work_dir: work_root().join(unique_run_name()),
                    quantum_ns: self.sim.quantum_ns,
                    icount_shift: self.sim.icount_shift,
                    uart,
                    natives,
                    rtl,
                    board: self.board.name.clone(),
                    pl_name: if self.backends.pl == "xsim" {
                        "xsim".to_string()
                    } else if self.native.is_empty() {
                        "none".to_string()
                    } else {
                        "native".to_string()
                    },
                };
                let mut engine = crate::QemuEngine::new(&spec)?;
                if let Some(m) = &self.media {
                    let video = m
                        .video_in
                        .as_deref()
                        .map(|v| parse_video_spec(v, base))
                        .transpose()?;
                    let audio = m
                        .audio_in
                        .as_deref()
                        .map(|a| parse_audio_spec(a, base))
                        .transpose()?;
                    if video.is_some() || audio.is_some() {
                        use crate::Engine;
                        engine.command(crate::Command::SetMediaSource { video, audio })?;
                    }
                }
                Ok(Box::new(engine))
            }
            other => Err(ApiError::unsupported(format!(
                "PS backend `{other}` is not implemented"
            ))),
        }
    }

    /// The simulator configuration this project asks for.
    pub fn sim_config(&self) -> SimConfig {
        SimConfig {
            board: self.board.name.clone(),
            seed: self.sim.seed,
            quantum_ps: self.sim.quantum_ns.saturating_mul(1000),
        }
    }

    /// Checks the project against the schema and this build, reporting every
    /// finding. `base` is the directory relative paths resolve against.
    pub fn validate(&self, base: &Path) -> Vec<Diagnostic> {
        let mut d = Vec::new();
        if self.schema_version != 1 {
            d.push(err(
                "schema_version",
                format!("is {}, only 1 is supported", self.schema_version),
            ));
        }
        let board = BoardDef::builtin(&self.board.name);
        if board.is_none() {
            d.push(err(
                "board.name",
                format!("unknown board `{}`; built-in: zybo-z7-20", self.board.name),
            ));
        }
        if let (Some(b), Some(ddr)) = (&board, self.board.ddr_mb) {
            if ddr != b.ddr_mb {
                d.push(warn(
                    "board.ddr_mb",
                    format!("{ddr} differs from the board's {} MiB", b.ddr_mb),
                ));
            }
        }

        for (field, value, valid, built) in [
            (
                "backends.ps",
                &self.backends.ps,
                PS_BACKENDS,
                IMPLEMENTED_PS,
            ),
            (
                "backends.pl",
                &self.backends.pl,
                PL_BACKENDS,
                IMPLEMENTED_PL,
            ),
        ] {
            if !valid.contains(&value.as_str()) {
                d.push(err(
                    field,
                    format!(
                        "unknown backend `{value}`; expected one of {}",
                        valid.join(", ")
                    ),
                ));
            } else if !built.contains(&value.as_str()) {
                d.push(err(
                    field,
                    format!(
                        "backend `{value}` is not implemented in this build; \
                         available: {} (see docs/STATUS.md)",
                        built.join(", ")
                    ),
                ));
            }
        }
        if self.backends.pl == "xsim" {
            match &self.rtl {
                Some(r) if !r.files.is_empty() && r.top.is_some() => {
                    for f in &r.files {
                        if f.contains('*') {
                            d.push(err(
                                "rtl.files",
                                format!("`{f}`: globs are not supported yet; list the files"),
                            ));
                        } else if !resolve(base, f).is_file() {
                            d.push(err("rtl.files", format!("file `{f}` does not exist")));
                        }
                    }
                }
                _ => d.push(err(
                    "rtl",
                    "backends.pl = \"xsim\" needs [rtl] with `files` and `top`",
                )),
            }
            if self.backends.ps == "demo" {
                d.push(err(
                    "backends.pl",
                    "an RTL design needs a PS that issues bus accesses; set `backends.ps = \"qemu\"`",
                ));
            }
            if !self.native.is_empty() {
                d.push(err(
                    "native",
                    "native PL models and an RTL design cannot be combined yet",
                ));
            }
        }
        if self.backends.pl == "native" && self.backends.ps == "demo" {
            d.push(err(
                "backends.pl",
                "native PL models need a PS that issues bus accesses; set `backends.ps = \"qemu\"`",
            ));
        }
        match self.host.get("uart1").map(String::as_str) {
            None | Some("file") | Some("tcp") => {}
            Some(other) => d.push(err(
                "host.uart1",
                format!(
                    "`{other}` is not valid; expected `file` (deterministic, output only) or `tcp`"
                ),
            )),
        }
        for (inst, model) in &self.native {
            if !crate::qemu_engine::NATIVE_MODELS.contains(&model.as_str()) {
                d.push(err(
                    &format!("native.{inst}"),
                    format!(
                        "unknown native model `{model}`; available: {}",
                        crate::qemu_engine::NATIVE_MODELS.join(", ")
                    ),
                ));
            }
            if self.native_placement(inst).is_none() {
                d.push(err(
                    &format!("native.{inst}"),
                    format!(
                        "needs a [connect] entry such as `\"{inst}.s_axi\" = \"M_AXI_GP0@0x43C00000\"`"
                    ),
                ));
            }
        }
        if !self.native.is_empty() && self.backends.pl != "native" {
            d.push(warn(
                "native",
                "ignored: set `backends.pl = \"native\"` to use native PL models",
            ));
        }

        if self.sim.quantum_ns == 0 {
            d.push(err("sim.quantum_ns", "must be non-zero"));
        }

        if let Some(boot) = &self.boot {
            if boot.mode != "elf" && boot.mode != "boot_bin" {
                d.push(err(
                    "boot.mode",
                    format!("`{}` is not valid; expected `elf` or `boot_bin`", boot.mode),
                ));
            }
        }
        let uses_cpu = self.backends.ps != "demo";
        for (field, cpu) in [("cpu0.elf", &self.cpu0), ("cpu1.elf", &self.cpu1)] {
            match cpu {
                Some(c) => {
                    if !resolve(base, &c.elf).is_file() {
                        d.push(err(field, format!("file `{}` does not exist", c.elf)));
                    }
                    if !uses_cpu {
                        d.push(warn(
                            field,
                            "ignored: the `demo` PS backend runs no software",
                        ));
                    }
                }
                None if field == "cpu0.elf" && uses_cpu => {
                    d.push(err(
                        "cpu0.elf",
                        "is required when a CPU backend is selected",
                    ));
                }
                None => {}
            }
        }

        if let Some(rtl) = &self.rtl {
            if !rtl.files.is_empty() && rtl.top.is_none() {
                d.push(err("rtl.top", "is required when rtl.files is set"));
            }
            if self.backends.pl == "demo" && !rtl.files.is_empty() {
                d.push(warn(
                    "rtl.files",
                    "ignored: the `demo` PL backend runs no RTL",
                ));
            }
        }

        for (key, value) in &self.connect {
            if let Some(msg) = check_connection(value, board.as_ref()) {
                d.push(err(&format!("connect.\"{key}\""), msg));
            }
        }
        d
    }
}

/// Where engines keep their scratch directories (QEMU logs, the XSim build):
/// `ZYNQSIM_WORK_DIR`, else `zynqsim` in the system temporary directory, never
/// the project tree. Directories left behind by crashed runs are removed once
/// they are a day old.
fn work_root() -> PathBuf {
    let root = std::env::var_os("ZYNQSIM_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zynqsim"));
    if let Ok(rd) = std::fs::read_dir(&root) {
        let day = std::time::Duration::from_secs(24 * 3600);
        for e in rd.flatten() {
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > day);
            if stale && e.file_name().to_string_lossy().starts_with("run-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    root
}

/// A run directory name no other engine in this or another process shares.
fn unique_run_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "run-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn resolve(base: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Validates one `[connect]` target; returns a message when it is wrong.
fn check_connection(value: &str, board: Option<&BoardDef>) -> Option<String> {
    let (port, addr) = match value.split_once('@') {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (value.trim(), None),
    };
    if let Some(a) = addr {
        let ok = a
            .strip_prefix("0x")
            .or_else(|| a.strip_prefix("0X"))
            .is_some_and(|h| !h.is_empty() && u64::from_str_radix(h, 16).is_ok());
        if !ok {
            return Some(format!("address `{a}` is not a hex number like 0x43C00000"));
        }
    }
    let indexed = |prefix: &str, limit: u32| -> Option<bool> {
        let n = port.strip_prefix(prefix)?.strip_suffix(']')?;
        Some(n.parse::<u32>().is_ok_and(|n| n < limit))
    };
    match port {
        "M_AXI_GP0" | "M_AXI_GP1" => {
            if addr.is_none() {
                return Some(format!(
                    "`{port}` needs a base address, e.g. {port}@0x43C00000"
                ));
            }
            return None;
        }
        "S_AXI_GP0" | "S_AXI_GP1" | "S_AXI_HP0" | "S_AXI_HP1" | "S_AXI_HP2" | "S_AXI_HP3"
        | "S_AXI_ACP" | "FCLK_CLK0" | "FCLK_CLK1" | "FCLK_CLK2" | "FCLK_CLK3" | "FCLK_RESET0_N"
        | "FCLK_RESET1_N" | "FCLK_RESET2_N" | "FCLK_RESET3_N" => {
            if addr.is_some() {
                return Some(format!("`{port}` does not take an address"));
            }
            return None;
        }
        _ => {}
    }
    if let Some(ok) = indexed("IRQ_F2P[", 16) {
        return (!ok).then(|| "IRQ_F2P index must be 0..15".to_string());
    }
    if let Some(ok) = indexed("EMIO[", 64) {
        return (!ok).then(|| "EMIO index must be 0..63".to_string());
    }
    if let Some(b) = board {
        let known = b.switches.iter().any(|s| s.id == port)
            || b.buttons.iter().any(|s| s.id == port)
            || b.leds.iter().any(|s| s.id == port)
            || b.rgb_leds.iter().any(|l| {
                ["_r", "_g", "_b"]
                    .iter()
                    .any(|s| port == format!("{}{s}", l.id))
            })
            || b.pmods
                .iter()
                .any(|p| p.pin_names().iter().any(|n| n == port));
        if known {
            return None;
        }
    }
    Some(format!(
        "`{port}` is not a PS port (M_AXI_GP0/1, S_AXI_GP0/1, S_AXI_HP0-3, S_AXI_ACP, \
         IRQ_F2P[n], FCLK_CLK0-3, FCLK_RESET0-3_N, EMIO[n]) or a board pin"
    ))
}

/// A starter project for `zynqsim init`.
pub fn starter_project_toml() -> &'static str {
    "# ZynqSim project (schema in docs/DESIGN.md).\n\
     schema_version = 1\n\
     \n\
     [board]\n\
     name = \"zybo-z7-20\"\n\
     \n\
     [backends]\n\
     ps = \"demo\"   # \"qemu\" and \"native\" are not implemented yet\n\
     pl = \"demo\"   # \"verilator\" | \"icarus\" | \"xsim\" | \"native\" not implemented yet\n\
     \n\
     [sim]\n\
     quantum_ns = 1000\n\
     seed = 1\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
schema_version = 1
[board]
name = "zybo-z7-20"
ddr_mb = 1024
[sim]
seed = 7
[connect]
"u_accel.s_axi" = "M_AXI_GP0@0x43C00000"
"u_accel.m_axi" = "S_AXI_HP0"
"u_accel.irq"   = "IRQ_F2P[0]"
"u_gpio.led"    = "led0"
"#;

    #[test]
    fn good_project_validates_and_maps_to_config() {
        let p = Project::from_toml(GOOD).unwrap();
        assert!(p.validate(Path::new(".")).is_empty());
        let c = p.sim_config();
        assert_eq!((c.seed, c.quantum_ps), (7, 1_000_000));
    }

    #[test]
    fn starter_project_is_valid() {
        let p = Project::from_toml(starter_project_toml()).unwrap();
        assert!(p.validate(Path::new(".")).is_empty());
    }

    #[test]
    fn unknown_keys_and_bad_toml_are_rejected_with_location() {
        let e = Project::from_toml("schema_version = 1\n[board]\nname = \"x\"\nbogus = 1\n")
            .unwrap_err();
        assert!(e.message.contains("bogus"), "{}", e.message);
        assert!(Project::from_toml("schema_version = ").is_err());
    }

    #[test]
    fn every_problem_is_reported_with_its_field() {
        let p = Project::from_toml(
            r#"
schema_version = 2
[board]
name = "nope"
[backends]
ps = "native"
pl = "magic"
[boot]
mode = "tftp"
[rtl]
files = ["rtl/*.v"]
[connect]
"a.b" = "M_AXI_GP0"
"c.d" = "M_AXI_GP0@zzz"
"e.f" = "IRQ_F2P[16]"
"g.h" = "S_AXI_HP4"
"#,
        )
        .unwrap();
        let d = p.validate(Path::new("."));
        let fields: Vec<&str> = d.iter().map(|x| x.field.as_str()).collect();
        for want in [
            "schema_version",
            "board.name",
            "backends.ps",
            "backends.pl",
            "boot.mode",
            "cpu0.elf",
            "rtl.top",
            "connect.\"a.b\"",
            "connect.\"c.d\"",
            "connect.\"e.f\"",
            "connect.\"g.h\"",
        ] {
            assert!(fields.contains(&want), "missing {want} in {fields:?}");
        }
        let ps = d.iter().find(|x| x.field == "backends.ps").unwrap();
        assert!(ps.message.contains("not implemented in this build"));
        assert!(ps.message.contains("available: demo, qemu"));
    }

    #[test]
    fn missing_elf_is_an_error_only_when_used() {
        let p = Project::from_toml(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[cpu0]\nelf = \"nope.elf\"\n",
        )
        .unwrap();
        let d = p.validate(Path::new("."));
        // Existence is checked even for the demo backend, plus an "ignored" warning.
        assert!(
            d.iter()
                .any(|x| x.field == "cpu0.elf" && x.severity == Severity::Error)
        );
        assert!(
            d.iter()
                .any(|x| x.field == "cpu0.elf" && x.severity == Severity::Warning)
        );
    }
}
