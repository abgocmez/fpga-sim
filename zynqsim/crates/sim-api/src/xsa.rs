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
    BackendsSection, BoardSection, BootSection, CpuSection, Diagnostic, Project, RtlSection,
    Severity, SimSection,
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
            IpClass::Ps7 | IpClass::Infrastructure { .. } => {}
            IpClass::VendorIp { title, .. } => unmodelled.push(mk(
                "needs a `vendor-ip` model (milestone P3, not built yet)".to_string(),
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

    let rtl = match &tb {
        Some((_, _)) => {
            let mut list: Vec<String> = srcs.iter().map(|s| esc(&s.project_path)).collect();
            list.push("tb/cosim_top.sv".to_string());
            Some(RtlSection {
                files: list,
                top: Some("cosim_top".to_string()),
                defines: Vec::new(),
                waves: false,
            })
        }
        None => None,
    };
    if let Some((_, text)) = &tb {
        files.push(("tb/cosim_top.sv".to_string(), text.clone().into_bytes()));
    }

    let project = Project {
        schema_version: 1,
        board: BoardSection {
            name: "zybo-z7-20".to_string(),
            ddr_mb: None,
        },
        backends: BackendsSection {
            ps: "qemu".to_string(),
            pl: if tb.is_some() { "xsim" } else { "demo" }.to_string(),
        },
        sim: SimSection::default(),
        boot: Some(BootSection {
            mode: "elf".to_string(),
        }),
        cpu0: Some(CpuSection { elf: esc(&elf) }),
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
        native: BTreeMap::new(),
        media: None,
        debug: None,
        network: None,
        storage: None,
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
            IpClass::Infrastructure { title } => format!("wiring: {title}"),
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
        } else if mapped_user.contains(&ip.instance) {
            format!(
                "RTL in XSim (module `{}`)",
                tb_module.unwrap_or(&ip.modtype)
            )
        } else if matches!(ip.class, IpClass::Infrastructure { .. }) {
            "wiring only: not simulated (the windows and clocks it routes are listed on the endpoints)"
                .to_string()
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
        _ => {
            let _ = writeln!(r, "* No `[rtl]`: no user RTL module was mapped.");
        }
    }
    if !rtl_sources.is_empty() {
        let _ = writeln!(r, "* RTL sources considered: {}.", rtl_sources.join(", "));
    }
    let _ = writeln!(
        r,
        "* `cpu0.elf = \"{}\"` (the project does not validate while this file is missing).",
        project.cpu0.as_ref().map_or("", |c| c.elf.as_str()),
    );
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
            "`{}` are in the archive but are not executed; the project boots the ELF directly \
             (`boot.mode = \"elf\"`), so firmware that depends on state the FSBL would have set \
             sees QEMU's defaults",
            handoff.archive.ps7_init.join("`, `")
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

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}
