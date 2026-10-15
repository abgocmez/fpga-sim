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
    /// (`0x100614`), ELF symbols (`main`) or source lines (`main.rs:42`, needs
    /// DWARF line info in the ELF).
    #[serde(default)]
    pub breakpoints: Vec<String>,
    /// Extra directories (relative to the project file) searched for the
    /// source files the ELF's line table names, besides the compilation
    /// directory recorded in it and the project directory.
    #[serde(default)]
    pub source_dirs: Vec<String>,
}

/// `[storage]` section: what sits in the board's microSD slot (PS SDIO0) and
/// QSPI flash socket. See ADR-012.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSection {
    /// A raw SD card image (MBR plus FAT; power-of-two size), relative to the
    /// project file. Build one with `zynqsim sd-image`.
    #[serde(default)]
    pub sd_image: Option<String>,
    /// Alternative to `sd_image`: a folder that is turned into a card image
    /// when the simulation starts (the folder is never written to).
    #[serde(default)]
    pub sd_dir: Option<String>,
    /// Card size in MiB for `sd_dir` (power of two, 8..=1024); the smallest
    /// that fits when absent.
    #[serde(default)]
    pub sd_size_mb: Option<u32>,
    /// A raw 16 MiB QSPI flash image. Build one with `zynqsim qspi-image`.
    #[serde(default)]
    pub qspi_image: Option<String>,
    /// Alternative to `qspi_image`: a file that is packed into an erased
    /// 16 MiB flash image at `qspi_offset` when the simulation starts.
    #[serde(default)]
    pub qspi_data: Option<String>,
    /// Byte offset of `qspi_data` in the flash (default 0; TOML allows
    /// `0x100000`).
    #[serde(default)]
    pub qspi_offset: Option<u64>,
    /// QEMU flash model: `s25fl129p0` (default, Spansion 128 Mbit),
    /// `s25fl129p1` or `n25q128a13` (Micron).
    #[serde(default)]
    pub qspi_model: Option<String>,
    /// Let guest writes change the image files. Default `false`: writes go to a
    /// throw-away overlay and the files are never modified.
    #[serde(default)]
    pub writable: bool,
}

/// `[network]` section: the board's PS Gigabit Ethernet (GEM0) and the
/// simulated LAN it is plugged into (ADR-011).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkSection {
    /// Attach the NIC. `false` keeps the section but leaves GEM0 unconnected.
    #[serde(default = "yes")]
    pub enable: bool,
    /// MAC address given to GEM0 (its address register's reset value).
    /// Default `00:0a:35:00:1e:53`.
    #[serde(default)]
    pub mac: Option<String>,
    /// The simulated LAN, `a.b.c.d/len` with a prefix of 8 to 27.
    /// Default `10.0.2.0/24`.
    #[serde(default)]
    pub subnet: Option<String>,
    /// The LAN's router/host/DHCP-server address, which answers ARP and ping.
    /// Default: the subnet's address plus 2.
    #[serde(default)]
    pub gateway: Option<String>,
    /// Isolate the guest from the machine's real network (default). `false`
    /// lets it reach hosts the machine can reach, which is not deterministic.
    #[serde(default = "yes")]
    pub restrict: bool,
}

fn yes() -> bool {
    true
}

impl NetworkSection {
    /// The network configuration this section asks for, or every problem
    /// found as `(field, message)` pairs.
    pub fn config(&self) -> Result<backend_qemu::net::NetConfig, Vec<(String, String)>> {
        use backend_qemu::net::{NetConfig, parse_cidr, parse_mac};
        use std::net::Ipv4Addr;
        let mut problems: Vec<(String, String)> = Vec::new();
        let mut cfg = NetConfig {
            restrict: self.restrict,
            ..NetConfig::default()
        };
        if let Some(m) = &self.mac {
            match parse_mac(m) {
                Ok(v) => cfg.mac = v,
                Err(e) => problems.push(("network.mac".into(), e)),
            }
        }
        if let Some(s) = &self.subnet {
            match parse_cidr(s) {
                Ok((net, len)) => {
                    cfg.subnet = net;
                    cfg.prefix = len;
                    if (8..=27).contains(&len) {
                        let mask = u32::MAX << (32 - u32::from(len));
                        cfg.gateway = Ipv4Addr::from((u32::from(net) & mask).wrapping_add(2));
                    }
                }
                Err(e) => problems.push(("network.subnet".into(), e)),
            }
        }
        if let Some(g) = &self.gateway {
            match g.trim().parse::<Ipv4Addr>() {
                Ok(a) => cfg.gateway = a,
                Err(_) => problems.push((
                    "network.gateway".into(),
                    format!("`{g}` is not an IPv4 address like 10.0.2.2"),
                )),
            }
        }
        // Consistency checks use the defaults for fields that failed to
        // parse, so skip a finding whose own input was already rejected, and
        // the address checks when the network itself is unusable.
        let rejected = |problems: &[(String, String)], f: &str| {
            problems.iter().any(|(p, _)| p == &format!("network.{f}"))
        };
        let subnet_unusable = rejected(&problems, "subnet");
        for (f, m) in cfg.problems() {
            if rejected(&problems, f) || (subnet_unusable && f == "gateway") {
                continue;
            }
            problems.push((format!("network.{f}"), m));
        }
        if problems.is_empty() {
            Ok(cfg)
        } else {
            Err(problems)
        }
    }
}

/// A `project.toml` (prompt §12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Where the project came from (`zynqsim init --xsa`) and which IP has no model.
    #[serde(default)]
    pub xsa: Option<crate::xsa::XsaSection>,
    /// Board devices attached to connectors.
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
    /// Initial physical quantities of the attached devices, by connector:
    /// `[device_values.pmod_ja] temperature = 23.5`.
    #[serde(default)]
    pub device_values: BTreeMap<String, BTreeMap<String, f64>>,
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
    /// The board's Ethernet and its simulated LAN.
    #[serde(default)]
    pub network: Option<NetworkSection>,
    /// microSD card and QSPI flash.
    #[serde(default)]
    pub storage: Option<StorageSection>,
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
                let mut devices = Vec::new();
                for (k, v) in &self.devices {
                    if let Some(port) = pmod_port(k) {
                        devices.push((port, v.clone()));
                    }
                }
                let mut device_values = Vec::new();
                for (k, vals) in &self.device_values {
                    if let Some(port) = pmod_port(k) {
                        for (name, v) in vals {
                            device_values.push((port, name.clone(), *v));
                        }
                    }
                }
                let spec = crate::QemuSpec {
                    devices,
                    device_values,
                    debug: debug.enabled,
                    breakpoints: debug.breakpoints.clone(),
                    source_dirs: std::iter::once(base.to_path_buf())
                        .chain(debug.source_dirs.iter().map(|d| resolve(base, d)))
                        .collect(),
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
                    net: match &self.network {
                        Some(n) if n.enable => Some(n.config().map_err(|p| {
                            ApiError::invalid_argument(
                                p.iter()
                                    .map(|(f, m)| format!("{f}: {m}"))
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            )
                        })?),
                        _ => None,
                    },
                    storage: self.storage_spec(base)?,
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

        if let Some(n) = &self.network {
            if n.enable {
                if self.backends.ps != "qemu" {
                    d.push(err(
                        "network",
                        "the board's Ethernet is a PS peripheral and needs `backends.ps = \"qemu\"`",
                    ));
                }
                if let Err(problems) = n.config() {
                    for (field, message) in problems {
                        d.push(err(&field, message));
                    }
                }
                if !n.restrict {
                    d.push(warn(
                        "network.restrict",
                        "the guest can reach the real network: traffic and timing then depend \
                         on the host, and runs are not reproducible",
                    ));
                }
            } else if n.mac.is_some() || n.subnet.is_some() || n.gateway.is_some() {
                d.push(warn(
                    "network",
                    "ignored: `enable = false` leaves GEM0 unconnected",
                ));
            }
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
        for (k, v) in &self.devices {
            let field = format!("devices.{k}");
            match pmod_port(k) {
                None => d.push(err(
                    &field,
                    "unknown connector; Pmod ports are pmod_ja, pmod_jb, pmod_jc, pmod_jd, pmod_je, \
                     the camera connector is cam",
                )),
                Some(port) => {
                    if devices::KINDS.contains(&v.as_str()) && !devices::fits_port(v, port) {
                        d.push(err(
                            &field,
                            if v == "pcam" {
                                "the Pcam plugs into the camera connector: `[devices] cam = \"pcam\"`"
                                    .to_string()
                            } else {
                                format!("`{v}` is a Pmod module: use pmod_ja..pmod_je")
                            },
                        ));
                    } else if !devices::KINDS.contains(&v.as_str()) {
                        d.push(err(
                            &field,
                            format!(
                                "unknown device `{v}`; available: {}",
                                devices::KINDS.join(", ")
                            ),
                        ));
                    } else if self.backends.pl != "xsim" {
                        d.push(warn(
                            &field,
                            "Pmod pins are driven only by an RTL design in XSim (backends.pl = \"xsim\")",
                        ));
                    }
                }
            }
        }
        for (k, vals) in &self.device_values {
            let field = format!("device_values.{k}");
            if pmod_port(k).is_none() {
                d.push(err(
                    &field,
                    "unknown connector; Pmod ports are pmod_ja, pmod_jb, pmod_jc, pmod_jd, pmod_je, \
                     the camera connector is cam",
                ));
                continue;
            }
            let Some(kind) = self.devices.get(k) else {
                d.push(err(
                    &field,
                    format!("no device is attached to `{k}`; add `[devices] {k} = \"...\"`"),
                ));
                continue;
            };
            let Some(dev) = devices::create(kind) else {
                continue; // reported under `devices.*`
            };
            let known = dev.values();
            for (name, v) in vals {
                let field = format!("device_values.{k}.{name}");
                match known.iter().find(|q| q.name.eq_ignore_ascii_case(name)) {
                    None => d.push(err(
                        &field,
                        format!(
                            "`{kind}` has no quantity `{name}`; available: {}",
                            if known.is_empty() {
                                "(none)".to_string()
                            } else {
                                known.iter().map(|q| q.name).collect::<Vec<_>>().join(", ")
                            }
                        ),
                    )),
                    Some(_) if !v.is_finite() => d.push(err(&field, "must be a finite number")),
                    Some(q) if *v < q.min || *v > q.max => d.push(warn(
                        &field,
                        format!(
                            "{v} is outside {}..{} {} and will be clamped",
                            q.min, q.max, q.unit
                        ),
                    )),
                    Some(_) => {}
                }
            }
        }
        if let Some(s) = &self.storage {
            self.validate_storage(s, base, &mut d);
        }
        if let Some(x) = &self.xsa {
            x.validate(&mut d);
        }
        d
    }

    fn validate_storage(&self, s: &StorageSection, base: &Path, d: &mut Vec<Diagnostic>) {
        use crate::storage as st;
        if self.backends.ps != "qemu" {
            d.push(warn(
                "storage",
                "ignored: only the `qemu` PS backend has an SD slot and QSPI flash",
            ));
        }
        if s.sd_image.is_some() && s.sd_dir.is_some() {
            d.push(err(
                "storage.sd_dir",
                "cannot be combined with `sd_image`: give one card source",
            ));
        }
        if let Some(f) = &s.sd_image {
            if let Err(why) = st::check_sd_file(&resolve(base, f)) {
                d.push(err("storage.sd_image", format!("`{f}` {why}")));
            }
        }
        if let Some(f) = &s.sd_dir {
            if !resolve(base, f).is_dir() {
                d.push(err(
                    "storage.sd_dir",
                    format!("`{f}` is not a directory (or does not exist)"),
                ));
            }
        }
        if let Some(mb) = s.sd_size_mb {
            if s.sd_dir.is_none() {
                d.push(warn(
                    "storage.sd_size_mb",
                    "ignored: it applies to `sd_dir`",
                ));
            } else if let Err(e) = st::check_card_size(mb) {
                d.push(err("storage.sd_size_mb", e.message));
            }
        }
        if s.qspi_image.is_some() && s.qspi_data.is_some() {
            d.push(err(
                "storage.qspi_data",
                "cannot be combined with `qspi_image`: give one flash source",
            ));
        }
        if let Some(f) = &s.qspi_image {
            if let Err(why) = st::check_qspi_file(&resolve(base, f)) {
                d.push(err("storage.qspi_image", format!("`{f}` {why}")));
            }
        }
        if let Some(f) = &s.qspi_data {
            match std::fs::metadata(resolve(base, f)) {
                Ok(m) if m.is_file() => {
                    let end = s.qspi_offset.unwrap_or(0).saturating_add(m.len());
                    if end > st::QSPI_BYTES as u64 {
                        d.push(err(
                            "storage.qspi_data",
                            format!(
                                "`{f}` ({} bytes) at offset {:#x} ends past the 16 MiB flash",
                                m.len(),
                                s.qspi_offset.unwrap_or(0)
                            ),
                        ));
                    }
                }
                _ => d.push(err(
                    "storage.qspi_data",
                    format!("`{f}` does not exist or is not a file"),
                )),
            }
        } else if s.qspi_offset.is_some() {
            d.push(warn(
                "storage.qspi_offset",
                "ignored: it applies to `qspi_data`",
            ));
        }
        let has_qspi = s.qspi_image.is_some() || s.qspi_data.is_some();
        if let Some(m) = &s.qspi_model {
            if !st::QSPI_MODELS.contains(&m.as_str()) {
                d.push(err(
                    "storage.qspi_model",
                    format!(
                        "unknown flash model `{m}`; available: {}",
                        st::QSPI_MODELS.join(", ")
                    ),
                ));
            } else if !has_qspi {
                d.push(warn(
                    "storage.qspi_model",
                    "ignored: no `qspi_image` or `qspi_data`",
                ));
            }
        }
        if s.writable {
            if s.sd_image.is_none() && s.qspi_image.is_none() {
                d.push(warn(
                    "storage.writable",
                    "no effect: a card or flash built from `sd_dir` / `qspi_data` is rebuilt \
                     for every run and the host files are never written",
                ));
            } else {
                d.push(warn(
                    "storage.writable",
                    "guest writes will change the image file(s); snapshot restore, reset and \
                     recording are refused for such a session (it cannot be repeated)",
                ));
            }
        }
    }

    /// The SD card and QSPI flash this project attaches. A folder card is
    /// built into an image here, once.
    fn storage_spec(&self, base: &Path) -> Result<crate::StorageSpec, ApiError> {
        let mut spec = crate::StorageSpec::default();
        let Some(s) = &self.storage else {
            return Ok(spec);
        };
        let file_name = |p: &Path, fallback: &str| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| fallback.to_string())
        };
        spec.writable = s.writable;
        spec.qspi_model = s.qspi_model.clone();
        if let Some(f) = &s.sd_image {
            spec.sd = Some(crate::ImageSource::File(resolve(base, f)));
        }
        if let Some(f) = &s.sd_dir {
            let dir = resolve(base, f);
            let image = crate::storage::build_sd_image(
                &dir,
                &crate::storage::SdImageOptions {
                    size_mb: s.sd_size_mb,
                    ..Default::default()
                },
            )?;
            spec.sd = Some(crate::ImageSource::Generated {
                name: format!("dir:{}", file_name(&dir, f)),
                image: std::sync::Arc::new(image),
            });
        }
        if let Some(f) = &s.qspi_image {
            spec.qspi = Some(crate::ImageSource::File(resolve(base, f)));
        }
        if let Some(f) = &s.qspi_data {
            let path = resolve(base, f);
            let data = std::fs::read(&path)
                .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
            let image = crate::storage::build_qspi_image(
                &data,
                s.qspi_offset.unwrap_or(0),
                (crate::storage::QSPI_BYTES >> 20) as u32,
            )?;
            spec.qspi = Some(crate::ImageSource::Generated {
                name: format!("file:{}", file_name(&path, f)),
                image: std::sync::Arc::new(image),
            });
        }
        Ok(spec)
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

/// `pmod_ja` .. `pmod_je` to the port index 0..=4, `cam` (the camera
/// connector) to 5.
pub(crate) fn pmod_port(key: &str) -> Option<u8> {
    match key {
        "cam" => Some(5),
        "pmod_ja" => Some(0),
        "pmod_jb" => Some(1),
        "pmod_jc" => Some(2),
        "pmod_jd" => Some(3),
        "pmod_je" => Some(4),
        _ => None,
    }
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
    fn devices_are_validated_by_connector_and_kind() {
        let base = "schema_version = 1
[board]
name = \"zybo-z7-20\"
";
        let ok = Project::from_toml(&format!(
            "{base}[backends]
ps = \"qemu\"
pl = \"xsim\"
[devices]
pmod_ja = \"oled\"
"
        ))
        .unwrap();
        let findings = ok.validate(Path::new("."));
        assert!(
            findings.iter().all(|d| !d.field.starts_with("devices")),
            "{findings:?}"
        );

        let bad = Project::from_toml(&format!(
            "{base}[devices]
pmod_jz = \"oled\"
pmod_jb = \"toaster\"
pmod_jc = \"oled\"
"
        ))
        .unwrap();
        let findings = bad.validate(Path::new("."));
        let msg = |f: &str| {
            findings
                .iter()
                .find(|d| d.field == f)
                .map(|d| (d.severity, d.message.clone()))
        };
        let (sev, m) = msg("devices.pmod_jz").expect("unknown connector");
        assert_eq!(sev, Severity::Error);
        assert!(m.contains("pmod_ja"), "{m}");
        let (sev, m) = msg("devices.pmod_jb").expect("unknown device");
        assert_eq!(sev, Severity::Error);
        assert!(m.contains("oled"), "{m}");
        // A valid device on a non-RTL PL is allowed but flagged.
        let (sev, _) = msg("devices.pmod_jc").expect("warning");
        assert_eq!(sev, Severity::Warning);
    }

    #[test]
    fn the_pcam_plugs_into_the_camera_connector_only() {
        let base = "schema_version = 1
[board]
name = \"zybo-z7-20\"
[backends]
ps = \"qemu\"
pl = \"xsim\"
";
        let ok = Project::from_toml(&format!(
            "{base}[devices]\ncam = \"pcam\"\npmod_jb = \"tmp2\"\n"
        ))
        .unwrap();
        assert!(
            ok.validate(Path::new("."))
                .iter()
                .all(|d| !d.field.starts_with("devices")),
            "{:?}",
            ok.validate(Path::new("."))
        );
        let bad = Project::from_toml(&format!(
            "{base}[devices]\npmod_ja = \"pcam\"\ncam = \"oled\"\n"
        ))
        .unwrap();
        let f = bad.validate(Path::new("."));
        let m = |field: &str| {
            f.iter()
                .find(|d| d.field == field)
                .map(|d| d.message.clone())
        };
        assert!(m("devices.pmod_ja").unwrap().contains("camera connector"));
        assert!(m("devices.cam").unwrap().contains("Pmod module"));
    }

    #[test]
    fn device_values_are_checked_against_the_device() {
        let base = "schema_version = 1
[board]
name = \"zybo-z7-20\"
[backends]
ps = \"qemu\"
pl = \"xsim\"
[devices]
pmod_ja = \"tmp2\"
pmod_jb = \"als\"
";
        let p = Project::from_toml(&format!(
            "{base}[device_values.pmod_ja]
temperature = 23.5
[device_values.pmod_jb]
light = 300
humidity = 1
[device_values.pmod_jc]
light = 1
[device_values.pmod_jz]
light = 1
"
        ))
        .unwrap();
        let f = p.validate(Path::new("."));
        let msg = |field: &str| {
            f.iter()
                .find(|d| d.field == field)
                .map(|d| (d.severity, d.message.clone()))
        };
        assert!(msg("device_values.pmod_ja.temperature").is_none(), "{f:?}");
        let (sev, m) = msg("device_values.pmod_jb.humidity").expect("unknown quantity");
        assert_eq!(sev, Severity::Error);
        assert!(m.contains("light"), "{m}");
        let (sev, m) = msg("device_values.pmod_jb.light").expect("out of range");
        assert_eq!(sev, Severity::Warning);
        assert!(m.contains("clamped"), "{m}");
        assert_eq!(
            msg("device_values.pmod_jc").expect("no device").0,
            Severity::Error
        );
        assert_eq!(
            msg("device_values.pmod_jz").expect("no connector").0,
            Severity::Error
        );
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

    fn storage_project(body: &str) -> Project {
        Project::from_toml(&format!(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[storage]\n{body}"
        ))
        .unwrap()
    }

    #[test]
    fn storage_section_is_validated_with_field_paths() {
        let d = std::env::temp_dir().join(format!("zs-proj-storage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("card")).unwrap();
        std::fs::write(d.join("good.img"), vec![0u8; 8 << 20]).unwrap();
        std::fs::write(d.join("odd.img"), vec![0u8; 10 << 20]).unwrap();
        std::fs::write(d.join("flash.bin"), vec![0xFFu8; 16 << 20]).unwrap();
        std::fs::write(d.join("short.bin"), vec![0xFFu8; 1 << 20]).unwrap();

        // A good project has no findings apart from the ignored-by-demo-PS note.
        let p = storage_project("sd_image = \"good.img\"\nqspi_image = \"flash.bin\"\n");
        let findings = p.validate(&d);
        assert!(
            findings.iter().all(|f| f.severity == Severity::Warning),
            "{findings:?}"
        );

        let p = storage_project(
            "sd_image = \"missing.img\"\nsd_dir = \"card\"\nqspi_image = \"short.bin\"\n\
             qspi_model = \"w99x\"\nsd_size_mb = 12\n",
        );
        let findings = p.validate(&d);
        let field = |f: &str| findings.iter().find(|x| x.field == f);
        assert!(
            field("storage.sd_image")
                .unwrap()
                .message
                .contains("does not exist")
        );
        assert!(
            field("storage.sd_dir")
                .unwrap()
                .message
                .contains("sd_image")
        );
        assert!(
            field("storage.qspi_image")
                .unwrap()
                .message
                .contains("16777216")
        );
        assert!(
            field("storage.qspi_model")
                .unwrap()
                .message
                .contains("s25fl129p0")
        );
        // `sd_size_mb` is only checked when `sd_dir` is the card source.
        let p = storage_project("sd_dir = \"card\"\nsd_size_mb = 12\n");
        assert!(
            p.validate(&d)
                .iter()
                .any(|x| x.field == "storage.sd_size_mb" && x.severity == Severity::Error)
        );

        let p = storage_project("sd_image = \"odd.img\"\n");
        let e = p.validate(&d);
        assert!(
            e.iter()
                .any(|x| x.field == "storage.sd_image" && x.message.contains("power-of-two")),
            "{e:?}"
        );

        let p = storage_project("sd_image = \"good.img\"\nwritable = true\n");
        assert!(p.validate(&d).iter().any(|x| x.field == "storage.writable"
            && x.severity == Severity::Warning
            && x.message.contains("refused")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn storage_round_trips_through_toml_and_rejects_unknown_keys() {
        let p = storage_project("sd_dir = \"card\"\nwritable = false\n");
        let back = Project::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(back.storage, p.storage);
        assert!(
            Project::from_toml(
                "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[storage]\nsd = \"x\"\n"
            )
            .is_err()
        );
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

    const NET_BASE: &str = "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n\
        [backends]\nps = \"qemu\"\n[cpu0]\nelf = \"fw.elf\"\n";

    fn net_findings(extra: &str) -> Vec<(String, Severity)> {
        let dir = std::env::temp_dir().join(format!("zs-netproj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fw.elf"), b"x").unwrap();
        let p = Project::from_toml(&format!("{NET_BASE}{extra}")).unwrap();
        p.validate(&dir)
            .into_iter()
            .map(|d| (d.field, d.severity))
            .collect()
    }

    #[test]
    fn network_section_defaults_and_overrides_become_a_configuration() {
        let p = Project::from_toml(&format!("{NET_BASE}[network]\n")).unwrap();
        let n = p.network.unwrap();
        assert!(n.enable && n.restrict);
        let c = n.config().unwrap();
        assert_eq!(c, backend_qemu::net::NetConfig::default());

        // A subnet alone moves the gateway with it (network address + 2).
        let n: NetworkSection =
            toml::from_str("subnet = \"192.168.50.0/24\"\nmac = \"02:aa:bb:cc:dd:ee\"").unwrap();
        let c = n.config().unwrap();
        assert_eq!(c.gateway.to_string(), "192.168.50.2");
        assert_eq!(c.mac, [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]);
        assert!(c.restrict);
    }

    #[test]
    fn network_section_problems_are_reported_with_their_fields() {
        assert!(net_findings("[network]\n").is_empty());
        let f =
            net_findings("[network]\nmac = \"nope\"\nsubnet = \"10.0.2.0/33\"\ngateway = \"x\"\n");
        for want in ["network.mac", "network.subnet", "network.gateway"] {
            assert!(
                f.iter().any(|(n, s)| n == want && *s == Severity::Error),
                "{f:?}"
            );
        }
        // Consistency between the fields is checked once each parses.
        let f = net_findings("[network]\nsubnet = \"10.0.2.0/24\"\ngateway = \"10.0.9.2\"\n");
        assert!(f.iter().any(|(n, _)| n == "network.gateway"), "{f:?}");
        // Open networking is allowed but flagged.
        let f = net_findings("[network]\nrestrict = false\n");
        assert_eq!(f, vec![("network.restrict".to_string(), Severity::Warning)]);
        // Disabled sections ignore their settings, with a note.
        let f = net_findings("[network]\nenable = false\nmac = \"zz\"\n");
        assert_eq!(f, vec![("network".to_string(), Severity::Warning)]);
        // Unknown keys are rejected like everywhere else.
        assert!(Project::from_toml(&format!("{NET_BASE}[network]\nspeed = 100\n")).is_err());
    }

    #[test]
    fn the_nic_needs_a_cpu_backend() {
        let p =
            Project::from_toml("schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[network]\n")
                .unwrap();
        let d = p.validate(Path::new("."));
        assert!(
            d.iter()
                .any(|x| x.field == "network" && x.severity == Severity::Error),
            "{d:?}"
        );
    }
}
