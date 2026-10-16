//! `zynqsim init --xsa`: a Vivado hardware handoff becomes a project (prompt section 12).
//!
//! The reader is the `xsa-import` crate (re-exported here); this module maps a
//! [`Design`] onto a [`Project`]: the user's RTL becomes `[rtl]` with a generated testbench,
//! the address map, interrupts and clocks become `[connect]`, and every IP that has no model is
//! recorded in `[xsa.unmodelled]`, which makes the project fail validation with a diagnostic
//! naming it. A design is never reported as runnable while an IP is unmodelled.

pub use xsa_import::*;

use crate::error::ApiError;
use crate::project::{
    BackendsSection, BoardSection, BootSection, CpuSection, Diagnostic, NativeParam, Project,
    RtlSection, Severity, SimSection,
};
use crate::tbgen;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// `[xsa]` section: provenance and the IP instances that have no model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XsaSection {
    /// The handoff this project was made from (informational).
    #[serde(default)]
    pub source: Option<String>,
    /// The import report written next to the project.
    #[serde(default)]
    pub report: Option<String>,
    /// IP instances without a model. Each one is a validation **error**: the run stops with
    /// the diagnostic. Remove an entry only if the firmware never touches that block.
    #[serde(default)]
    pub unmodelled: Vec<UnmodelledIp>,
}

/// One IP instance the project cannot simulate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnmodelledIp {
    /// Instance name in the block design.
    pub instance: String,
    /// `VLNV` of the IP.
    pub vlnv: String,
    /// Human title (`AXI GPIO`).
    #[serde(default)]
    pub title: Option<String>,
    /// Base address in the PS address map (`0x41200000`).
    #[serde(default)]
    pub base: Option<String>,
    /// Size in bytes of the address window.
    #[serde(default)]
    pub size: Option<u64>,
    /// `IRQ_F2P` bits the instance drives.
    #[serde(default)]
    pub irq: Vec<u32>,
    /// Why it cannot run.
    pub reason: String,
}

impl UnmodelledIp {
    /// The diagnostic text: instance, VLNV, address and reason.
    pub fn message(&self) -> String {
        let mut s = String::new();
        if let Some(t) = &self.title {
            let _ = write!(s, "{t} ");
        }
        let _ = write!(s, "`{}` ({})", self.instance, self.vlnv);
        if let Some(b) = &self.base {
            let _ = write!(s, " at {b}");
            if let Some(n) = self.size {
                let _ = write!(s, " (+{n:#x})");
            }
        }
        if !self.irq.is_empty() {
            let bits: Vec<String> = self.irq.iter().map(|b| format!("IRQ_F2P[{b}]")).collect();
            let _ = write!(s, ", drives {}", bits.join(", "));
        }
        let _ = write!(s, ": {}", self.reason);
        s
    }
}

impl XsaSection {
    /// Appends one error per unmodelled IP instance.
    pub(crate) fn validate(&self, d: &mut Vec<Diagnostic>) {
        for u in &self.unmodelled {
            d.push(Diagnostic {
                severity: Severity::Error,
                field: format!("xsa.unmodelled.{}", u.instance),
                message: u.message(),
            });
        }
    }
}

/// Options of [`plan`].
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// User RTL files (Verilog/SystemVerilog) in addition to the ones inside the archive.
    pub rtl: Vec<PathBuf>,
    /// Firmware ELF for core 0 (default `fw/app.elf`).
    pub elf: Option<String>,
    /// Firmware ELF for core 1.
    pub elf1: Option<String>,
    /// A Vivado file list (`.prj`, `.f`) whose HDL files are added to the RTL.
    pub filelist: Option<PathBuf>,
    /// A `BOOT.BIN` (bootgen output) the project boots instead of a `[cpu0] elf`: the image is
    /// validated, copied to `BOOT.BIN` in the project directory and `[boot] mode = "boot_bin"`
    /// is written (ADR-018, ADR-019). `elf` is then ignored.
    pub boot_bin: Option<PathBuf>,
    /// A Vivado `export_simulation` folder (or its `.prj`/`.f`): the user RTL and its testbench
    /// come from the export (`[rtl] export_dir`, ADR-017) instead of `rtl` files and a generated
    /// testbench; the Xilinx IP of the handoff still becomes vendor-ip models. If the export
    /// has no testbench of its own (a source calling the `rp_*` DPI functions), one is generated
    /// for the user module as with `rtl`. Not combinable with `rtl` or `filelist`.
    pub export: Option<PathBuf>,
}

/// A planned import: nothing is written until [`XsaImport::write`].
#[derive(Clone, Debug)]
pub struct XsaImport {
    /// The project that will be written.
    pub project: Project,
    /// `project.toml` text.
    pub project_toml: String,
    /// `xsa-report.md` text.
    pub report: String,
    /// Files to create, relative to the project directory.
    pub files: Vec<(String, Vec<u8>)>,
    /// IP instances without a model (the same list is in `project.xsa`).
    pub unmodelled: Vec<UnmodelledIp>,
    /// The user module the testbench drives, if any.
    pub testbench_module: Option<String>,
    /// Things worth knowing that are not errors.
    pub warnings: Vec<String>,
    /// The parsed handoff.
    pub handoff: Handoff,
}

impl XsaImport {
    /// `true` when no IP instance is unmodelled and a testbench exists or no PL is needed.
    pub fn runnable_design(&self) -> bool {
        self.unmodelled.is_empty()
    }

    /// Writes the project directory. Refuses to overwrite `project.toml`, the report or the
    /// testbench. Returns the files created.
    pub fn write(&self, dir: &Path) -> Result<Vec<PathBuf>, ApiError> {
        for (rel, _) in &self.files {
            let p = dir.join(rel);
            if p.exists() {
                return Err(ApiError::invalid_argument(format!(
                    "{} already exists; refusing to overwrite",
                    p.display()
                )));
            }
        }
        let mut created = Vec::new();
        for (rel, bytes) in &self.files {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ApiError::io(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&p, bytes)
                .map_err(|e| ApiError::io(format!("cannot write {}: {e}", p.display())))?;
            created.push(p);
        }
        Ok(created)
    }
}

/// Where an AXI DMA's stream interfaces lead.
enum Streams {
    /// `M_AXIS_MM2S` reaches `S_AXIS_S2MM` of the same DMA, directly or through the listed
    /// AXI4-Stream plumbing instances.
    Loopback(Vec<String>),
    /// The stream interfaces are not connected to another instance.
    Open,
    /// Connected to this instance, which is neither the DMA nor transparent plumbing.
    Peer(String),
}

/// True for the AXI4-Stream plumbing IP the loopback trace sees through.
fn stream_plumbing(ip: &IpInstance) -> bool {
    matches!(ip.class, IpClass::Infrastructure { .. }) && ip.modtype.starts_with("axis_")
}

fn is_master_kind(kind: &str) -> bool {
    matches!(kind, "MASTER" | "INITIATOR")
}

/// Follows the DMA's `M_AXIS_MM2S` (and checks the `S_AXIS_S2MM` source) through the bus
/// interfaces of the handoff.
fn dma_streams(design: &Design, dma: &IpInstance) -> Streams {
    let bus_of = |ip: &IpInstance, name: &str| {
        ip.interfaces
            .iter()
            .find(|i| i.name == name)
            .and_then(|i| i.bus_name.clone())
    };
    let mut via: Vec<String> = Vec::new();
    let mut bus = bus_of(dma, "M_AXIS_MM2S");
    for _ in 0..8 {
        let Some(b) = bus.take() else { break };
        // The other end(s) of this bus: slave-side interfaces on any instance.
        let ends: Vec<(&IpInstance, &BusInterface)> = design
            .ips
            .iter()
            .flat_map(|ip| ip.interfaces.iter().map(move |i| (ip, i)))
            .filter(|(ip, i)| {
                i.bus_name.as_deref() == Some(b.as_str())
                    && !is_master_kind(&i.kind)
                    && !(ip.instance == dma.instance && i.name == "M_AXIS_MM2S")
            })
            .collect();
        let Some((ip, iface)) = ends.first() else {
            break;
        };
        if ends.len() > 1 {
            return Streams::Peer(
                ends.iter()
                    .map(|(ip, _)| ip.instance.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if ip.instance == dma.instance && iface.name == "S_AXIS_S2MM" {
            return Streams::Loopback(via);
        }
        if stream_plumbing(ip) {
            via.push(ip.instance.clone());
            let masters: Vec<&BusInterface> = ip
                .interfaces
                .iter()
                .filter(|i| {
                    is_master_kind(&i.kind)
                        && i.vlnv.as_deref().is_some_and(|v| v.contains(":axis:"))
                })
                .collect();
            if masters.len() == 1 {
                bus = masters[0].bus_name.clone();
                continue;
            }
        }
        return Streams::Peer(ip.instance.clone());
    }
    // Nothing downstream of MM2S: is something upstream of S2MM?
    if let Some(b) = bus_of(dma, "S_AXIS_S2MM") {
        let src = design.ips.iter().find(|ip| {
            ip.instance != dma.instance
                && ip
                    .interfaces
                    .iter()
                    .any(|i| i.bus_name.as_deref() == Some(b.as_str()) && is_master_kind(&i.kind))
        });
        if let Some(ip) = src {
            return Streams::Peer(ip.instance.clone());
        }
    }
    Streams::Open
}

/// The instance that uses the second port of the block memory behind an AXI BRAM controller,
/// if any (the model keeps the memory inside the controller).
fn bram_port_b_user(design: &Design, ctrl: &IpInstance) -> Option<String> {
    let bus = ctrl
        .interfaces
        .iter()
        .find(|i| i.name == "BRAM_PORTA")
        .and_then(|i| i.bus_name.clone())?;
    let mem = design.ips.iter().find(|ip| {
        ip.instance != ctrl.instance
            && ip
                .interfaces
                .iter()
                .any(|i| i.bus_name.as_deref() == Some(bus.as_str()))
    })?;
    let portb = mem
        .interfaces
        .iter()
        .find(|i| i.name == "BRAM_PORTB")
        .and_then(|i| i.bus_name.clone())?;
    design
        .ips
        .iter()
        .find(|ip| {
            ip.instance != mem.instance
                && ip
                    .interfaces
                    .iter()
                    .any(|i| i.bus_name.as_deref() == Some(portb.as_str()))
        })
        .map(|ip| ip.instance.clone())
}

/// The Pmod letter (`a`..`e`) a lower-case external port name says: `pmod_ja`, `jb_iic`.
fn pmod_letter(name: &str) -> Option<char> {
    let n = name.to_ascii_lowercase();
    if let Some(i) = n.find("pmod_j") {
        let c = n.as_bytes().get(i + 6).copied()?;
        if (b'a'..=b'e').contains(&c) {
            return Some(char::from(c));
        }
    }
    for tok in n.split(|c: char| !c.is_ascii_alphanumeric()) {
        if let [b'j', c @ b'a'..=b'e'] = tok.as_bytes() {
            return Some(char::from(*c));
        }
    }
    None
}

fn hex(v: u64) -> String {
    format!("{v:#010X}")
}

/// Plans an import of `xsa` (an `.xsa` or `.hwh`) into the project directory `out`.
///
/// `out` is only used to compute relative paths; nothing is written.
pub fn plan(xsa: &Path, out: &Path, opts: &ImportOptions) -> Result<XsaImport, ApiError> {
    let handoff = xsa_import::read(xsa).map_err(|e| ApiError::invalid_argument(e.0))?;
    let design = &handoff.design;
    let Some(ps) = design.ps7.as_ref() else {
        return Err(ApiError::unsupported(format!(
            "{} has no processing_system7 instance: only Zynq-7000 designs are supported",
            xsa.display()
        )));
    };
    let mut warnings: Vec<String> = Vec::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    // ---- RTL sources: archive, --rtl, --filelist ------------------------------------------
    struct Src {
        label: String,
        text: String,
        /// Path as it goes into `[rtl].files` (relative to the project directory).
        project_path: String,
    }
    let mut srcs: Vec<Src> = Vec::new();
    for f in &opts.rtl {
        let text = std::fs::read_to_string(f)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", f.display())))?;
        srcs.push(Src {
            label: f.display().to_string(),
            text,
            project_path: tbgen::relative_path(out, f),
        });
    }
    if let Some(list) = &opts.filelist {
        let text = std::fs::read_to_string(list)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", list.display())))?;
        let dir = list.parent().unwrap_or(Path::new("."));
        for e in xsa_import::parse_filelist(&text) {
            let lower = e.path.to_ascii_lowercase();
            if !(lower.ends_with(".v") || lower.ends_with(".sv")) {
                warnings.push(format!(
                    "file list entry `{}` is not Verilog/SystemVerilog and is not passed to XSim",
                    e.path
                ));
                continue;
            }
            let p = if Path::new(&e.path).is_absolute() {
                PathBuf::from(&e.path)
            } else {
                dir.join(&e.path)
            };
            match std::fs::read_to_string(&p) {
                Ok(text) => srcs.push(Src {
                    label: p.display().to_string(),
                    text,
                    project_path: tbgen::relative_path(out, &p),
                }),
                Err(err) => warnings.push(format!(
                    "file list entry `{}` cannot be read ({err}); skipped",
                    e.path
                )),
            }
        }
    }
    // The export: its sources are only read here (to find the user module and any testbench of
    // its own); `[rtl] export_dir` makes XSim compile them.
    let mut export_rel: Option<String> = None;
    let mut export_own_tb_top: Option<String> = None;
    if let Some(exp) = &opts.export {
        if !opts.rtl.is_empty() || opts.filelist.is_some() {
            return Err(ApiError::invalid_argument(
                "--export-sim cannot be combined with --rtl or --filelist (the export lists the \
                 RTL)",
            ));
        }
        let ed = crate::exportsim::read_export(exp).map_err(ApiError::invalid_argument)?;
        let errors: Vec<String> = ed
            .diagnostics
            .iter()
            .filter(|d| d.severity == xsim_export::Severity::Error)
            .map(ToString::to_string)
            .collect();
        if !errors.is_empty() {
            return Err(ApiError::invalid_argument(format!(
                "the export cannot be used:\n  {}",
                errors.join("\n  ")
            )));
        }
        warnings.extend(
            ed.diagnostics
                .iter()
                .filter(|d| d.severity == xsim_export::Severity::Warning)
                .map(ToString::to_string),
        );
        let tops = ed.top_modules();
        for f in ed.sources.iter().filter(|f| !f.lang.is_vhdl()) {
            let Ok(text) = std::fs::read_to_string(&f.path) else {
                continue;
            };
            if export_own_tb_top.is_none() && (text.contains("rp_open") || text.contains("rp_next"))
            {
                let names = crate::exportinit::module_names(&text);
                export_own_tb_top = tops
                    .iter()
                    .find(|n| names.contains(n))
                    .cloned()
                    .or_else(|| names.first().cloned());
            }
            srcs.push(Src {
                label: format!("{} (in the export)", f.path.display()),
                text,
                project_path: String::new(),
            });
        }
        export_rel = Some(tbgen::relative_path(out, exp));
    }
    for s in &handoff.archive.sources {
        let lower = s.path.to_ascii_lowercase();
        if matches!(s.kind, SourceKind::Vhdl) {
            warnings.push(format!(
                "`{}` is VHDL: XSim projects here compile Verilog/SystemVerilog only; not used",
                s.path
            ));
            continue;
        }
        if !(lower.ends_with(".v") || lower.ends_with(".sv")) {
            continue; // headers and file lists are not compile units
        }
        let rel = format!("rtl/{}", s.path);
        files.push((rel.clone(), s.text.clone().into_bytes()));
        srcs.push(Src {
            label: format!("{} (in the archive)", s.path),
            text: s.text.clone(),
            project_path: rel,
        });
    }

    // ---- Per-IP mapping ---------------------------------------------------------------------
    let mut unmodelled: Vec<UnmodelledIp> = Vec::new();
    let mut tb: Option<(String, String)> = None; // (module, testbench text)
    let mut tb_module_instance: Option<String> = None;
    let mut mapped_user: Vec<String> = Vec::new();
    let mut module_notes: Vec<String> = Vec::new();
    // Vendor IP the vendor-ip library models: instance -> model, parameters, board bindings.
    let mut native: BTreeMap<String, String> = BTreeMap::new();
    let mut native_params: BTreeMap<String, BTreeMap<String, NativeParam>> = BTreeMap::new();
    let mut native_bindings: BTreeMap<String, String> = BTreeMap::new();
    let mut claimed_signals: Vec<&'static str> = Vec::new();
    // Passive IP: recognised, carries no registers the firmware can reach, mapped to nothing.
    let mut passive: Vec<(String, String)> = Vec::new();

    for ip in &design.ips {
        let base = ip.address.first();
        let irq_bits: Vec<u32> = design
            .irqs_of(&ip.instance)
            .flat_map(|l| l.bit..l.bit + l.width)
            .collect();
        let mk = |reason: String, title: Option<String>| UnmodelledIp {
            instance: ip.instance.clone(),
            vlnv: ip.vlnv.clone(),
            title,
            base: base.map(|a| hex(a.base)),
            size: base.map(AddrRange::size),
            irq: irq_bits.clone(),
            reason,
        };
        match &ip.class {
            IpClass::Ps7 => {}
            IpClass::Infrastructure { title }
                if base.is_some_and(|a| {
                    matches!(a.master_interface.as_str(), "M_AXI_GP0" | "M_AXI_GP1")
                }) =>
            {
                // A passive-class IP with an address window of its own has registers the
                // firmware can reach (a Clocking Wizard with its AXI4-Lite interface, an
                // AXI4-Stream Switch with its control interface): not passive.
                unmodelled.push(mk(
                    format!(
                        "{title} with an AXI4-Lite register interface the firmware can reach; \
                         its registers are not modelled (its clocks come from the handoff)"
                    ),
                    Some(title.clone()),
                ));
            }
            IpClass::Infrastructure { title } => passive.push((ip.instance.clone(), title.clone())),
            IpClass::VendorIp { name, .. }
                if vendor_ip::is_model(name)
                    && base.is_some_and(|a| {
                        matches!(a.master_interface.as_str(), "M_AXI_GP0" | "M_AXI_GP1")
                    }) =>
            {
                let a = base.expect("checked above");
                let mut params: BTreeMap<String, NativeParam> = BTreeMap::new();
                let mut text: BTreeMap<String, String> = BTreeMap::new();
                for k in vendor_ip::param_names(name) {
                    if let Some(v) = ip.params.get(*k) {
                        text.insert((*k).to_string(), v.clone());
                    }
                }
                let hz = design
                    .clocks_of(&ip.instance)
                    .find_map(|c| c.hz)
                    .or_else(|| {
                        ip.params
                            .get("C_S_AXI_ACLK_FREQ_HZ")
                            .and_then(|v| vendor_ip::parse_number(v))
                    });
                if let Some(hz) = hz {
                    text.insert("CLK_FREQ_HZ".to_string(), hz.to_string());
                }
                for (k, v) in &text {
                    params.insert(
                        k.clone(),
                        match v.parse::<i64>() {
                            Ok(i) => NativeParam::Int(i),
                            Err(_) => NativeParam::Text(v.clone()),
                        },
                    );
                }
                params.insert(
                    "ADDRESS_SIZE".to_string(),
                    NativeParam::Text(format!("{:#x}", a.size())),
                );
                // Board bindings of a GPIO's channels, from the external ports' names.
                if name == "axi_gpio" {
                    let cfg = vendor_ip::gpio::GpioConfig::from_params(&text, a.size());
                    match cfg {
                        Err(e) => {
                            unmodelled.push(mk(
                                format!("AXI GPIO parameters are not usable: {e}"),
                                Some("AXI GPIO".to_string()),
                            ));
                            continue;
                        }
                        Ok(cfg) => {
                            for ch in 0..cfg.channels() {
                                let (iface, prefix) = if ch == 0 {
                                    ("GPIO", "gpio_io_")
                                } else {
                                    ("GPIO2", "gpio2_io_")
                                };
                                let hint = ip.ports.iter().find_map(|p| {
                                    if !p.name.starts_with(prefix) {
                                        return None;
                                    }
                                    p.connections.iter().find_map(|c| {
                                        let internal = c.instance == ps.instance
                                            || design.ip(&c.instance).is_some();
                                        (!internal).then(|| c.port.to_ascii_lowercase())
                                    })
                                });
                                let Some(hint) = hint else { continue };
                                let narrow = cfg.width[ch] <= 4;
                                let signal: Option<&'static str> = if hint.contains("led") {
                                    Some("board.leds")
                                } else if hint.contains("sw") {
                                    Some("board.sw")
                                } else if hint.contains("btn") || hint.contains("button") {
                                    Some("board.btn")
                                } else if narrow && cfg.all_outputs[ch] {
                                    Some("board.leds")
                                } else if narrow && cfg.all_inputs[ch] {
                                    Some("board.sw")
                                } else {
                                    None
                                };
                                match signal {
                                    Some(sig) if !claimed_signals.contains(&sig) => {
                                        claimed_signals.push(sig);
                                        native_bindings
                                            .insert(format!("{}.{iface}", ip.instance), sig.into());
                                        module_notes.push(format!(
                                            "`{}.{iface}` (external `{hint}`) is bound to {sig} \
                                             {}; change `[connect]` to rebind it",
                                            ip.instance,
                                            if hint.contains("led")
                                                || hint.contains("sw")
                                                || hint.contains("btn")
                                                || hint.contains("button")
                                            {
                                                "by the port's name"
                                            } else {
                                                "as a guess from its direction and width"
                                            }
                                        ));
                                    }
                                    Some(sig) => module_notes.push(format!(
                                        "`{}.{iface}` (external `{hint}`) would map to {sig}, \
                                         which another GPIO channel already uses: left unbound",
                                        ip.instance
                                    )),
                                    None => module_notes.push(format!(
                                        "`{}.{iface}` (external `{hint}`) is not bound to a \
                                         board signal: add `\"{}.{iface}\" = \"board.leds\"` \
                                         (or board.sw, board.btn, board.pmod_ja..je) to \
                                         `[connect]`",
                                        ip.instance, ip.instance
                                    )),
                                }
                            }
                        }
                    }
                }
                match name.as_str() {
                    "axi_dma" => {
                        // The memory path: the HP port the block design connects the masters to.
                        let port = ip
                            .master_windows
                            .iter()
                            .find(|w| {
                                w.instance == ps.instance && w.interface.starts_with("S_AXI_")
                            })
                            .map(|w| w.interface.clone());
                        match port {
                            Some(p) => {
                                let other = ip.master_windows.iter().any(|w| {
                                    w.instance == ps.instance
                                        && w.interface.starts_with("S_AXI_")
                                        && w.interface != p
                                });
                                if other {
                                    module_notes.push(format!(
                                        "`{}`: its memory masters reach the PS through different \
                                         ports; the model uses {p} for both",
                                        ip.instance
                                    ));
                                }
                                params.insert("PS_PORT".to_string(), NativeParam::Text(p));
                            }
                            None => module_notes.push(format!(
                                "`{}`: no memory window to the PS was found for its masters; \
                                 S_AXI_HP0 is assumed",
                                ip.instance
                            )),
                        }
                        match dma_streams(design, ip) {
                            Streams::Loopback(via) => {
                                params.insert("STREAM_LOOPBACK".to_string(), NativeParam::Int(1));
                                module_notes.push(format!(
                                    "`{}`: M_AXIS_MM2S feeds S_AXIS_S2MM{}: every MM2S packet \
                                     arrives at S2MM (STREAM_LOOPBACK)",
                                    ip.instance,
                                    if via.is_empty() {
                                        " directly".to_string()
                                    } else {
                                        format!(" through {}", via.join(", "))
                                    }
                                ));
                            }
                            Streams::Open => {
                                params.insert("STREAM_LOOPBACK".to_string(), NativeParam::Int(0));
                                module_notes.push(format!(
                                    "`{}`: its AXI4-Stream interfaces are not connected to \
                                     another IP: MM2S packets are only available to the host \
                                     and S2MM receives nothing",
                                    ip.instance
                                ));
                            }
                            Streams::Peer(peer) => {
                                unmodelled.push(mk(
                                    format!(
                                        "its AXI4-Stream side is connected to `{peer}`; the model \
                                         supports the stream side only as an MM2S to S2MM \
                                         loopback (a stream connection to user RTL in XSim or \
                                         to other IP does not exist)"
                                    ),
                                    Some("AXI DMA".to_string()),
                                ));
                                continue;
                            }
                        }
                    }
                    "axi_iic" => {
                        // The external IIC pins say which Pmod port the master sits on.
                        let hint = ip.ports.iter().find_map(|p| {
                            if !(p.name.starts_with("scl_") || p.name.starts_with("sda_")) {
                                return None;
                            }
                            p.connections.iter().find_map(|c| {
                                let internal =
                                    c.instance == ps.instance || design.ip(&c.instance).is_some();
                                (!internal).then(|| c.port.to_ascii_lowercase())
                            })
                        });
                        let group = hint.as_deref().and_then(pmod_letter);
                        match (group, hint) {
                            (Some(g), Some(h)) => {
                                let sig = format!("board.pmod_j{g}");
                                if claimed_signals.contains(&sig.as_str()) {
                                    module_notes.push(format!(
                                        "`{}.IIC` (external `{h}`) would map to {sig}, which \
                                         another channel already uses: left unbound",
                                        ip.instance
                                    ));
                                } else {
                                    native_bindings
                                        .insert(format!("{}.IIC", ip.instance), sig.clone());
                                    module_notes.push(format!(
                                        "`{}.IIC` (external `{h}`) is bound to {sig} by the port's \
                                         name (SCL on pin 3, SDA on pin 4 like the TMP2 module; \
                                         PMOD_SCL_BIT/PMOD_SDA_BIT in [native_params] change that)",
                                        ip.instance
                                    ));
                                }
                            }
                            (_, h) => module_notes.push(format!(
                                "`{}.IIC`{} is not bound to a board connector: nothing answers on \
                                 the bus (every address is NACKed). Add \
                                 `\"{}.IIC\" = \"board.pmod_ja\"` (or jb..je) to `[connect]` and a \
                                 module such as `pmod_ja = \"tmp2\"` to `[devices]`",
                                ip.instance,
                                h.map_or(String::new(), |h| format!(" (external `{h}`)")),
                                ip.instance
                            )),
                        }
                    }
                    "axi_bram_ctrl" => {
                        if let Some(other) = bram_port_b_user(design, ip) {
                            unmodelled.push(mk(
                                format!(
                                    "the block memory behind it is also used through its other \
                                     port by `{other}`; the model keeps the memory in the \
                                     controller only, so that sharing does not exist"
                                ),
                                Some("AXI BRAM Controller".to_string()),
                            ));
                            continue;
                        }
                    }
                    "axi_intc" => {
                        if let Some(src) = design.intc_inputs.iter().find_map(|l| {
                            (l.intc == ip.instance
                                && design
                                    .ip(&l.link.instance)
                                    .is_some_and(|s| matches!(s.class, IpClass::UserRtl { .. })))
                            .then(|| l.link.instance.clone())
                        }) {
                            unmodelled.push(mk(
                                format!(
                                    "its input is driven by the user RTL `{src}`, which runs in \
                                     XSim: interrupts of RTL cannot be delivered to a native \
                                     controller (connect the RTL interrupt to the PS instead)"
                                ),
                                Some("AXI Interrupt Controller".to_string()),
                            ));
                            continue;
                        }
                        let wide = design
                            .intc_inputs
                            .iter()
                            .filter(|l| l.intc == ip.instance && l.link.width > 1)
                            .map(|l| format!("{}.{}", l.link.instance, l.link.port))
                            .collect::<Vec<_>>();
                        if !wide.is_empty() {
                            unmodelled.push(mk(
                                format!(
                                    "multi-bit interrupt output(s) {} feed the controller; only \
                                     one-bit outputs are routed",
                                    wide.join(", ")
                                ),
                                Some("AXI Interrupt Controller".to_string()),
                            ));
                            continue;
                        }
                    }
                    _ => {}
                }
                native.insert(ip.instance.clone(), name.clone());
                native_params.insert(ip.instance.clone(), params);
            }
            IpClass::VendorIp { name, title } => unmodelled.push(mk(
                if vendor_ip::is_model(name) {
                    "has a vendor-ip model, but the instance is not reachable from \
                     M_AXI_GP0/M_AXI_GP1 in the address map"
                        .to_string()
                } else {
                    format!(
                        "needs a `vendor-ip` model that does not exist yet (the library has {})",
                        vendor_ip::MODELS.join(", ")
                    )
                },
                Some(title.clone()),
            )),
            IpClass::UnknownVendorIp => unmodelled.push(mk(
                format!(
                    "Xilinx IP `{}` is not in ZynqSim's vendor-ip catalog and has no model",
                    ip.modtype
                ),
                None,
            )),
            IpClass::ThirdParty => unmodelled.push(mk(
                format!(
                    "IP from `{}` has no model and its sources are not part of the handoff; \
                     pass plain RTL for it with --rtl if it is available",
                    ip.vlnv.split(':').next().unwrap_or("?")
                ),
                None,
            )),
            IpClass::UserRtl { .. } => {
                // Find the module's source.
                let found = srcs.iter().find_map(|s| {
                    tbgen::parse_module(&s.text, Some(&ip.modtype))
                        .ok()
                        .map(|m| (s, m))
                });
                let Some((src, module)) = found else {
                    unmodelled.push(mk(
                        format!(
                            "the RTL of module `{}` is not in the handoff; pass it with \
                             --rtl FILE (module references do not carry their sources)",
                            ip.modtype
                        ),
                        None,
                    ));
                    continue;
                };
                let axi = match tbgen::detect_axi_lite(&module) {
                    Ok(a) => a,
                    Err(e) => {
                        unmodelled.push(mk(
                            format!(
                                "module `{}` ({}) has no AXI4-Lite slave the testbench \
                                 generator understands: {e}; write a testbench by hand",
                                ip.modtype, src.label
                            ),
                            None,
                        ));
                        continue;
                    }
                };
                if tbgen::detect_axi_master(&module).is_err() {
                    unmodelled.push(mk(
                        format!(
                            "module `{}` has an AXI master the bridge cannot drive \
                             (only 32-bit AXI4 masters are bridged); write a testbench by hand",
                            ip.modtype
                        ),
                        None,
                    ));
                    continue;
                }
                if tb.is_none() && export_own_tb_top.is_some() && tb_module_instance.is_none() {
                    // The export brings its own testbench: it drives this module; nothing to
                    // generate (its interrupt wiring is the testbench's business).
                    module_notes.push(format!(
                        "`{}` (module `{}`) is driven by the export's own testbench `{}`: no \
                         testbench is generated and its interrupt outputs are not forwarded by \
                         ZynqSim",
                        ip.instance,
                        module.name,
                        export_own_tb_top.as_deref().unwrap_or("?")
                    ));
                    tb_module_instance = Some(ip.instance.clone());
                    mapped_user.push(ip.instance.clone());
                    continue;
                }
                if let Some((first, _)) = &tb {
                    unmodelled.push(mk(
                        format!(
                            "a second user RTL slave: the generated testbench drives one \
                             module (`{first}`); write a testbench that instantiates both"
                        ),
                        None,
                    ));
                    continue;
                }
                // IRQ_F2P bit of each interrupt output the generator forwards.
                let outs = tbgen::detect_irqs(&module, &axi);
                let bits: Vec<Option<u32>> = outs
                    .iter()
                    .map(|n| {
                        design
                            .irqs_of(&ip.instance)
                            .find(|l| &l.port == n && l.width == 1)
                            .map(|l| l.bit)
                    })
                    .collect();
                for (n, b) in outs.iter().zip(&bits) {
                    if b.is_none() {
                        module_notes.push(format!(
                            "`{}.{n}` looks like an interrupt but is not wired to IRQ_F2P in \
                             the block design: not forwarded",
                            ip.instance
                        ));
                    }
                }
                for l in design.irqs_of(&ip.instance) {
                    if !outs.contains(&l.port) {
                        module_notes.push(format!(
                            "`{}.{}` drives IRQ_F2P[{}] in the block design but the testbench \
                             generator only forwards outputs named *irq*/*interrupt*/*intr*: \
                             it is NOT forwarded, edit the testbench",
                            ip.instance, l.port, l.bit
                        ));
                    }
                }
                let mut text = tbgen::generate_testbench_with_irqs(&module, &axi, Some(&bits));
                // The testbench clock is the design's clock, not a fixed 100 MHz.
                if let Some(hz) = design
                    .clocks_of(&ip.instance)
                    .find(|c| c.port == axi.clock)
                    .and_then(|c| c.hz)
                {
                    if hz != 100_000_000 {
                        let half_ns = 5.0e8 / hz as f64;
                        let half = format!("{half_ns:.3}")
                            .trim_end_matches('0')
                            .trim_end_matches('.')
                            .to_string();
                        text = text.replace(
                            "always #5 aclk = ~aclk;   // 100 MHz = FCLK_CLK0",
                            &format!(
                                "always #{half} aclk = ~aclk;   // {} MHz, from the block design",
                                hz as f64 / 1e6
                            ),
                        );
                    }
                }
                tb = Some((module.name.clone(), text));
                tb_module_instance = Some(ip.instance.clone());
                mapped_user.push(ip.instance.clone());
            }
        }
    }
    warnings.extend(module_notes);

    // ---- Project ----------------------------------------------------------------------------
    let mut connect: BTreeMap<String, String> = BTreeMap::new();
    for ip in &design.ips {
        if matches!(ip.class, IpClass::Infrastructure { .. }) {
            continue;
        }
        for a in &ip.address {
            if matches!(a.master_interface.as_str(), "M_AXI_GP0" | "M_AXI_GP1") {
                connect.insert(
                    format!("{}.{}", ip.instance, a.interface),
                    format!("{}@{}", a.master_interface, hex(a.base)),
                );
            }
        }
        for w in &ip.master_windows {
            if w.instance == ps.instance
                && (w.interface.starts_with("S_AXI_HP")
                    || w.interface.starts_with("S_AXI_GP")
                    || w.interface == "S_AXI_ACP")
            {
                connect.insert(
                    format!("{}.{}", ip.instance, w.master_interface),
                    w.interface.clone(),
                );
            }
        }
        for l in design.irqs_of(&ip.instance) {
            if l.width == 1 {
                connect.insert(
                    format!("{}.{}", ip.instance, l.port),
                    format!("IRQ_F2P[{}]", l.bit),
                );
            } else {
                for k in 0..l.width {
                    connect.insert(
                        format!("{}.{}[{k}]", ip.instance, l.port),
                        format!("IRQ_F2P[{}]", l.bit + k),
                    );
                }
            }
        }
        for l in design.intc_inputs_of(&ip.instance) {
            if l.link.width == 1 {
                connect.insert(
                    format!("{}.{}", ip.instance, l.link.port),
                    format!("{}.intr[{}]", l.intc, l.link.bit),
                );
            }
        }
        for c in design.clocks_of(&ip.instance) {
            if let Some(n) = c.fclk {
                connect.insert(
                    format!("{}.{}", ip.instance, c.port),
                    format!("FCLK_CLK{n}"),
                );
            }
        }
        for r in design.resets.iter().filter(|r| r.instance == ip.instance) {
            if let Some(n) = r.fclk_reset {
                connect.insert(
                    format!("{}.{}", ip.instance, r.port),
                    format!("FCLK_RESET{n}_N"),
                );
            }
        }
    }

    connect.extend(native_bindings);

    // A BOOT.BIN replaces the ELF: validated here (an image ZynqSim could not run is refused,
    // as `init --boot` refuses it) and copied next to the project.
    let boot_init = match &opts.boot_bin {
        Some(bin) => {
            let b = crate::bootbin::plan_project(bin)?;
            files.push((b.image_name.clone(), b.loaded.bytes.clone()));
            if opts.elf.is_some() {
                warnings.push(
                    "--elf is ignored: the project boots the BOOT.BIN (`boot.mode = \"boot_bin\"`)"
                        .to_string(),
                );
            }
            Some(b)
        }
        None => None,
    };
    let elf = {
        let e = opts.elf.clone().unwrap_or_else(|| "fw/app.elf".to_string());
        if Path::new(&e).is_file() {
            tbgen::relative_path(out, Path::new(&e))
        } else {
            e
        }
    };
    let elf1 = opts.elf1.as_ref().map(|e| {
        if Path::new(e).is_file() {
            tbgen::relative_path(out, Path::new(e))
        } else {
            e.clone()
        }
    });
    let esc = |p: &str| p.replace('\\', "/");

    if export_rel.is_some() && tb.is_none() && export_own_tb_top.is_none() {
        return Err(ApiError::unsupported(
            "the export has no testbench that calls the ZynqSim `rp_*` DPI functions and the \
             handoff has no user AXI4-Lite RTL module found in it to generate one for; write a \
             testbench (docs/TEST_YOUR_DESIGN.md) and add it to the export's sim_1 set",
        ));
    }
    let rtl = match &tb {
        _ if export_rel.is_some() => Some(RtlSection {
            files: if tb.is_some() {
                vec!["tb/cosim_top.sv".to_string()]
            } else {
                Vec::new()
            },
            export_dir: export_rel.as_deref().map(esc),
            top: Some(
                export_own_tb_top
                    .clone()
                    .unwrap_or_else(|| "cosim_top".to_string()),
            ),
            ..RtlSection::default()
        }),
        Some((_, _)) => {
            let mut list: Vec<String> = srcs.iter().map(|s| esc(&s.project_path)).collect();
            list.push("tb/cosim_top.sv".to_string());
            Some(RtlSection {
                files: list,
                top: Some("cosim_top".to_string()),
                defines: Vec::new(),
                waves: false,
                ..RtlSection::default()
            })
        }
        None => None,
    };
    if let Some((_, text)) = &tb {
        files.push(("tb/cosim_top.sv".to_string(), text.clone().into_bytes()));
    }

    let project = Project {
        schema_version: 1,
        board: BoardSection::named("zybo-z7-20"),
        backends: BackendsSection {
            ps: "qemu".to_string(),
            pl: if tb.is_some() || export_rel.is_some() {
                "xsim"
            } else if !native.is_empty() {
                "native"
            } else {
                "demo"
            }
            .to_string(),
        },
        sim: SimSection::default(),
        boot: Some(match &boot_init {
            Some(b) => BootSection {
                mode: "boot_bin".to_string(),
                file: Some(b.image_name.clone()),
            },
            None => BootSection {
                mode: "elf".to_string(),
                file: None,
            },
        }),
        cpu0: boot_init.is_none().then(|| CpuSection { elf: esc(&elf) }),
        cpu1: elf1.as_ref().map(|e| CpuSection { elf: esc(e) }),
        rtl,
        connect,
        xsa: Some(XsaSection {
            source: Some(tbgen::relative_path(out, xsa)),
            report: Some("xsa-report.md".to_string()),
            unmodelled: unmodelled.clone(),
        }),
        devices: BTreeMap::new(),
        device_values: BTreeMap::new(),
        host: BTreeMap::new(),
        native,
        native_params,
        media: None,
        debug: None,
        network: None,
        storage: None,
        ps: None,
    };
    let body = project.to_toml()?;
    let project_toml = format!(
        "# Generated by `zynqsim init --xsa` from {} (design `{}`, Vivado {}).\n\
         # The full inventory, what was mapped and what was not is in xsa-report.md.\n\
         # [xsa.unmodelled] entries are validation errors: the run stops with the diagnostic.\n\
         {body}",
        xsa.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        design.name,
        design.vivado_version.as_deref().unwrap_or("?"),
    );
    let tb_module = tb.as_ref().map(|(m, _)| m.clone());
    let report = render_report(
        xsa,
        &handoff,
        &project,
        &unmodelled,
        tb_module.as_deref(),
        tb_module_instance.as_deref(),
        &mapped_user,
        &srcs.iter().map(|s| s.label.clone()).collect::<Vec<_>>(),
        &warnings,
        &passive,
    );
    files.push((
        "project.toml".to_string(),
        project_toml.clone().into_bytes(),
    ));
    files.push(("xsa-report.md".to_string(), report.clone().into_bytes()));
    Ok(XsaImport {
        project,
        project_toml,
        report,
        files,
        unmodelled,
        testbench_module: tb_module,
        warnings,
        handoff,
    })
}

#[allow(clippy::too_many_arguments)]
fn render_report(
    xsa: &Path,
    handoff: &Handoff,
    project: &Project,
    unmodelled: &[UnmodelledIp],
    tb_module: Option<&str>,
    tb_instance: Option<&str>,
    mapped_user: &[String],
    rtl_sources: &[String],
    warnings: &[String],
    passive: &[(String, String)],
) -> String {
    let d = &handoff.design;
    let ps = d.ps7.as_ref();
    let mut r = String::new();
    let _ = writeln!(r, "# XSA import report: `{}`\n", d.name);
    let _ = writeln!(
        r,
        "Source `{}`; Vivado {}; device {} {} {}; board {}.\n",
        xsa.display(),
        d.vivado_version.as_deref().unwrap_or("?"),
        d.arch.as_deref().unwrap_or("?"),
        d.device.as_deref().unwrap_or("?"),
        d.package.as_deref().unwrap_or(""),
        d.board.as_deref().unwrap_or("none"),
    );

    let _ = writeln!(r, "## Verdict\n");
    if unmodelled.is_empty() {
        let _ = writeln!(
            r,
            "No IP instance is unmodelled. This is **not** a claim that the design runs \
             correctly: nothing was run by the import. Run `zynqsim run --project project.toml` \
             and read its output. The PS clock tree, MIO and `ps7_init` are not simulated (see \
             *Not applied*).\n"
        );
    } else {
        let _ = writeln!(
            r,
            "**NOT RUNNABLE as a whole design: {} IP instance(s) have no model.** The project was \
             generated, but `zynqsim run` and `zynqsim inspect --project` stop with one \
             diagnostic per instance listed under *Not supported*. Nothing here was run.\n",
            unmodelled.len()
        );
    }

    if !passive.is_empty() {
        let _ = writeln!(
            r,
            "Passive IP: {} instance(s) ({}) are recognised and carry no registers the firmware \
             can reach, so they are mapped to nothing and are **not** errors.\n",
            passive.len(),
            passive
                .iter()
                .map(|(i, t)| format!("`{i}` {t}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let _ = writeln!(r, "## Processing system (PS7)\n");
    match ps {
        None => {
            let _ = writeln!(r, "No PS7 instance.\n");
        }
        Some(p) => {
            let _ = writeln!(r, "Instance `{}` (`{}`).\n", p.instance, p.vlnv);
            let _ = writeln!(
                r,
                "| FCLK | enabled | frequency | reset port |\n|---|---|---|---|"
            );
            for (i, f) in p.fclk.iter().enumerate() {
                let hz = f
                    .hz()
                    .map_or("-".to_string(), |h| format!("{} MHz", h as f64 / 1e6));
                let _ = writeln!(
                    r,
                    "| FCLK_CLK{i} | {} | {hz} | {} |",
                    yn(f.enabled),
                    yn(f.reset_enabled)
                );
            }
            let _ = writeln!(r);
            let on = |b: bool| if b { "yes" } else { "no" };
            let _ = writeln!(
                r,
                "AXI ports: M_AXI_GP0 {}, M_AXI_GP1 {}, S_AXI_GP0 {}, S_AXI_GP1 {}, S_AXI_HP0-3 {} / {} / {} / {}, \
                 S_AXI_ACP {}.\n",
                on(p.m_axi_gp[0]),
                on(p.m_axi_gp[1]),
                on(p.s_axi_gp[0]),
                on(p.s_axi_gp[1]),
                on(p.s_axi_hp[0]),
                on(p.s_axi_hp[1]),
                on(p.s_axi_hp[2]),
                on(p.s_axi_hp[3]),
                on(p.s_axi_acp),
            );
            let _ = writeln!(
                r,
                "IRQ_F2P: {}. EMIO GPIO: {}.\n",
                if p.irq_f2p_enabled {
                    format!("enabled, {} bit(s) wide", p.irq_f2p_width)
                } else {
                    "not used".to_string()
                },
                p.emio_gpio_width
                    .map_or("not used".to_string(), |w| format!("{w} bits")),
            );
            let _ = writeln!(r, "| Peripheral | enabled | IO |\n|---|---|---|");
            for x in &p.peripherals {
                let _ = writeln!(
                    r,
                    "| {} | {} | {} |",
                    x.name,
                    yn(x.enabled),
                    x.io.as_deref().unwrap_or("-")
                );
            }
            let _ = writeln!(r);
            if p.mio.is_empty() {
                let _ = writeln!(r, "MIO: the handoff has no MIO pin tree.\n");
            } else {
                let mut groups: Vec<(String, Vec<String>)> = Vec::new();
                for m in &p.mio {
                    if m.peripheral.is_empty() || m.peripheral == "unused" {
                        continue;
                    }
                    let entry = format!("{}({})", m.index, m.signal);
                    match groups.iter_mut().find(|(n, _)| *n == m.peripheral) {
                        Some((_, v)) => v.push(entry),
                        None => groups.push((m.peripheral.clone(), vec![entry])),
                    }
                }
                let _ = writeln!(r, "MIO use (pin(signal)):\n");
                for (n, v) in groups {
                    let _ = writeln!(r, "* {n}: {}", v.join(", "));
                }
                let _ = writeln!(r);
            }
        }
    }

    let _ = writeln!(r, "## PL instances\n");
    let _ = writeln!(
        r,
        "| Instance | VLNV | Class | Address | Size | IRQ_F2P | Clock | Disposition |\n|---|---|---|---|---|---|---|---|"
    );
    for ip in &d.ips {
        let class = match &ip.class {
            IpClass::Ps7 => "PS7".to_string(),
            IpClass::UserRtl { packaged: false } => "user RTL (module reference)".to_string(),
            IpClass::UserRtl { packaged: true } => "user RTL (packaged IP)".to_string(),
            IpClass::VendorIp { title, .. } => format!("vendor IP: {title}"),
            IpClass::Infrastructure { title } => format!("passive IP: {title}"),
            IpClass::UnknownVendorIp => "Xilinx IP, not in catalog".to_string(),
            IpClass::ThirdParty => "third-party IP".to_string(),
        };
        let addr = ip.address.first().map_or("-".to_string(), |a| hex(a.base));
        let size = ip
            .address
            .first()
            .map_or("-".to_string(), |a| format!("{:#x}", a.size()));
        let irq: Vec<String> = d
            .irqs_of(&ip.instance)
            .map(|l| {
                if l.width == 1 {
                    format!("[{}] (GIC {})", l.bit, IrqLink::gic_id(l.bit))
                } else {
                    format!("[{}..{}]", l.bit, l.bit + l.width - 1)
                }
            })
            .collect();
        let clk: Vec<String> = d
            .clocks_of(&ip.instance)
            .map(|c| {
                let src = c.fclk.map_or_else(
                    || c.source_instance.clone().unwrap_or_else(|| "?".to_string()),
                    |n| format!("FCLK_CLK{n}"),
                );
                format!(
                    "{} <- {src}{}",
                    c.port,
                    c.hz.map_or(String::new(), |h| format!(" @ {} MHz", h as f64 / 1e6))
                )
            })
            .collect();
        let disp = if let Some(u) = unmodelled.iter().find(|u| u.instance == ip.instance) {
            format!("**not supported**: {}", u.reason)
        } else if let Some(m) = project.native.get(&ip.instance) {
            format!(
                "vendor-ip model `{m}` (native, register level; unverified register facts are \
                 marked VERIFY in the model docs)"
            )
        } else if mapped_user.contains(&ip.instance) {
            format!(
                "RTL in XSim (module `{}`)",
                tb_module.unwrap_or(&ip.modtype)
            )
        } else if matches!(ip.class, IpClass::Infrastructure { .. }) {
            passive_disposition(&ip.modtype).to_string()
        } else {
            "-".to_string()
        };
        let _ = writeln!(
            r,
            "| `{}` | `{}` | {class} | {addr} | {size} | {} | {} | {disp} |",
            ip.instance,
            ip.vlnv,
            if irq.is_empty() {
                "-".to_string()
            } else {
                irq.join(", ")
            },
            if clk.is_empty() {
                "-".to_string()
            } else {
                clk.join("; ")
            },
        );
    }
    let _ = writeln!(r);

    let _ = writeln!(r, "## Address map (PS view)\n");
    let ranges: Vec<&AddrRange> = d
        .memory_map
        .iter()
        .filter(|a| ps.is_some_and(|p| p.instance == a.master))
        .collect();
    if ranges.is_empty() {
        let _ = writeln!(r, "No PL address windows.\n");
    } else {
        let _ = writeln!(
            r,
            "| Base | Last | Size | Master port | Slave | Interface |\n|---|---|---|---|---|---|"
        );
        let mut sorted = ranges;
        sorted.sort_by_key(|a| a.base);
        for a in sorted {
            let _ = writeln!(
                r,
                "| {} | {} | {:#x} | {} | `{}` | {} |",
                hex(a.base),
                hex(a.high),
                a.size(),
                a.master_interface,
                a.instance,
                a.interface
            );
        }
        let _ = writeln!(r);
    }

    let _ = writeln!(r, "## Project\n");
    let _ = writeln!(
        r,
        "* PS backend `{}`, PL backend `{}`.",
        project.backends.ps, project.backends.pl
    );
    match (&project.rtl, tb_module) {
        (Some(rtl), Some(m)) => {
            let _ = writeln!(
                r,
                "* `[rtl]`: top `cosim_top` (generated, `tb/cosim_top.sv`) instantiating `{m}` \
                 for block-design instance `{}`; the testbench answers every address of the \
                 PS's M_AXI_GP0 window with that module, so the address map below is \
                 informational for the RTL path.",
                tb_instance.unwrap_or("?")
            );
            for f in &rtl.files {
                let _ = writeln!(r, "  * `{f}`");
            }
        }
        (Some(rtl), None) if rtl.export_dir.is_some() => {
            let _ = writeln!(
                r,
                "* `[rtl] export_dir = \"{}\"`, top `{}`: the PL sources and the testbench are \
                 the Vivado simulation export's (compiled by XSim as the export lists them); the \
                 export's own testbench drives the user module for block-design instance `{}`. \
                 The export holds no Xilinx IP: those instances are the vendor-ip models below.",
                rtl.export_dir.as_deref().unwrap_or("?"),
                rtl.top.as_deref().unwrap_or("?"),
                tb_instance.unwrap_or("?")
            );
        }
        _ => {
            let _ = writeln!(r, "* No `[rtl]`: no user RTL module was mapped.");
        }
    }
    if !rtl_sources.is_empty() {
        let _ = writeln!(r, "* RTL sources considered: {}.", rtl_sources.join(", "));
    }
    match (&project.cpu0, &project.boot) {
        (None, Some(b)) if crate::bootbin::is_boot_bin_mode(&b.mode) => {
            let _ = writeln!(
                r,
                "* `[boot] mode = \"boot_bin\"`, `file = \"{}\"`: core 0 starts at the first \
                 application partition of the image. What a BOOT.BIN boot does **not** do:",
                b.file.as_deref().unwrap_or("?")
            );
            for n in crate::bootbin::NOTES {
                let _ = writeln!(r, "  * {n}");
            }
        }
        _ => {
            let _ = writeln!(
                r,
                "* `cpu0.elf = \"{}\"` (the project does not validate while this file is missing).",
                project.cpu0.as_ref().map_or("", |c| c.elf.as_str()),
            );
        }
    }
    if !project.native.is_empty() {
        let _ = writeln!(
            r,
            "* `[native]`: {} vendor-ip model(s) (`{}`). They answer in their own address \
             windows; every other PL access goes to the RTL (ADR-016). Interrupts reach the PS \
             at the next lock-step sync (at most one quantum late).",
            project.native.len(),
            project
                .native
                .iter()
                .map(|(i, m)| format!("{i}: {m}"))
                .collect::<Vec<_>>()
                .join("`, `")
        );
    }
    if !project.connect.is_empty() {
        let _ = writeln!(r, "\n`[connect]`:\n");
        for (k, v) in &project.connect {
            let _ = writeln!(r, "* `{k}` = `{v}`");
        }
    }
    let _ = writeln!(r);

    let _ = writeln!(r, "## Not supported\n");
    if unmodelled.is_empty() {
        let _ = writeln!(r, "Nothing is recorded as unmodelled.\n");
    } else {
        for u in unmodelled {
            let _ = writeln!(r, "* {}", u.message());
        }
        let _ = writeln!(r);
    }

    let _ = writeln!(r, "## Not applied\n");
    let _ = writeln!(
        r,
        "* **PS7 configuration is read, not applied.** QEMU's Zynq machine has every PS \
         peripheral; MIO routing, peripheral enables and the PS clock tree (PLLs, dividers) \
         do not change what runs."
    );
    let init = if handoff.archive.ps7_init.is_empty() {
        "the archive has no `ps7_init` files".to_string()
    } else {
        format!(
            "`{}` are in the archive but are not executed; the project starts the firmware \
             directly (`boot.mode = \"{}\"`; a BOOT.BIN's FSBL is not run either), so firmware \
             that depends on state the FSBL would have set sees QEMU's defaults",
            handoff.archive.ps7_init.join("`, `"),
            project.boot.as_ref().map_or("elf", |b| b.mode.as_str())
        )
    };
    let _ = writeln!(r, "* **`ps7_init`**: {init}.");
    if handoff.archive.has_bitstream {
        let _ = writeln!(
            r,
            "* **Bitstream** in the archive is not used: the PL is simulated from RTL."
        );
    }
    let fclk0_note = ps
        .and_then(|p| p.fclk[0].hz())
        .filter(|h| *h != 100_000_000)
        .map(|h| {
            format!(
                "* The generated testbench's clock follows the user module's clock port ({} MHz).",
                h as f64 / 1e6
            )
        });
    if let Some(n) = fclk0_note {
        let _ = writeln!(r, "{n}");
    }
    if ps.is_some_and(|p| p.s_axi_acp) {
        let _ = writeln!(
            r,
            "* S_AXI_ACP is enabled: cache coherency is not modelled."
        );
    }
    if ps.is_some_and(|p| p.emio_gpio_width.is_some()) {
        let _ = writeln!(
            r,
            "* EMIO GPIO is configured; EMIO pins are not connected to PL ports by this import."
        );
    }
    if ps.is_some_and(|p| p.irq_f2p_enabled) {
        let _ = writeln!(
            r,
            "* IRQ_F2P[n] to GIC ID: bits 0-7 are 61-68 and 8-15 are 84-91 (Zynq-7000 TRM); \
             QEMU's co-simulation device tree decides what the guest actually sees."
        );
    }
    let _ = writeln!(r);

    if !warnings.is_empty() || !d.notes.is_empty() {
        let _ = writeln!(r, "## Notes\n");
        for w in warnings.iter().chain(d.notes.iter()) {
            let _ = writeln!(r, "* {w}");
        }
        let _ = writeln!(r);
    }
    if !handoff.archive.entries.is_empty() {
        let _ = writeln!(r, "## Archive contents\n");
        for (n, s) in &handoff.archive.entries {
            let _ = writeln!(r, "* `{n}` ({s} bytes)");
        }
        let _ = writeln!(r);
    }
    r
}

/// What the report says about a passive IP instance.
fn passive_disposition(modtype: &str) -> &'static str {
    match modtype {
        "clk_wiz" => {
            "passive: clock source; its registers are not modelled and the clocks it makes come \
             from the handoff"
        }
        "blk_mem_gen" => {
            "passive: the memory contents live in the `axi_bram_ctrl` model in front of it"
        }
        t if t.starts_with("axis_") => {
            "passive: AXI4-Stream plumbing, transparent (an AXI DMA loopback through it is followed)"
        }
        _ => {
            "passive: wiring only, not simulated (the windows and clocks it routes are listed on \
             the endpoints)"
        }
    }
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}
