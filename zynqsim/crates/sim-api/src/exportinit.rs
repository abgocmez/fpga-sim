//! `zynqsim init --export-sim`: a project from a Vivado simulation export (ADR-017).
//!
//! The export is *referenced* from `[rtl]` (`export_dir` / `filelist`), not copied. When it
//! carries a ZynqSim testbench (calls the `rp_*` DPI functions) that is the top; otherwise a
//! testbench for the AXI4-Lite slave inside it (an RTL module or a `write_verilog -mode
//! funcsim` netlist) is generated, exactly as `init --ip` does.

use crate::ApiError;
use crate::exportsim::read_export;
use std::path::Path;
use xsim_export::{ExportDesign, Severity};

/// Options of [`plan_init`].
#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    /// Firmware ELF of core 0 (default `fw/app.elf`).
    pub elf: Option<String>,
    /// Firmware ELF of core 1.
    pub elf1: Option<String>,
    /// The module the testbench drives (default: a top of the export, else the first AXI4-Lite
    /// slave found).
    pub module: Option<String>,
    /// The base address for the comment in `project.toml`.
    pub base: Option<u64>,
}

/// A planned project: nothing is written until the caller does it.
#[derive(Clone, Debug)]
pub struct ExportInit {
    /// Files to create, relative to the project directory.
    pub files: Vec<(String, Vec<u8>)>,
    /// The `project.toml` text.
    pub project_toml: String,
    /// The read export.
    pub design: ExportDesign,
    /// The elaborated top module.
    pub top: String,
    /// The module the generated testbench drives (`None`: the export brings its own testbench).
    pub driven_module: Option<String>,
    /// The firmware path in `project.toml`.
    pub elf: String,
    /// Things worth knowing that are not errors (unsupported options, choices made).
    pub warnings: Vec<String>,
}

impl ExportInit {
    /// Writes the files into `dir`; refuses to overwrite any. Returns what was created.
    pub fn write(&self, dir: &Path) -> Result<Vec<std::path::PathBuf>, ApiError> {
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

/// Names of the `module X` declarations of a source text, in order.
fn module_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("module") {
        let before_ok = rest[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let tail = &rest[i + 6..];
        rest = tail;
        if !before_ok || !tail.starts_with(char::is_whitespace) {
            continue;
        }
        let n: String = tail
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !n.is_empty() && n != "endmodule" && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// Plans a project in `out` from the export at `export` (a folder, `.prj` or `.f`).
pub fn plan_init(export: &Path, out: &Path, opts: &InitOptions) -> Result<ExportInit, ApiError> {
    let design = read_export(export).map_err(ApiError::invalid_argument)?;
    let errors: Vec<String> = design
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(ToString::to_string)
        .collect();
    if !errors.is_empty() {
        return Err(ApiError::invalid_argument(format!(
            "the export cannot be used:\n  {}",
            errors.join("\n  ")
        )));
    }
    let mut warnings: Vec<String> = design
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(ToString::to_string)
        .collect();

    // Every Verilog/SystemVerilog source, read once (VHDL cannot hold the DUT's interface for
    // the generator and is compiled by the adapter as it is).
    let texts: Vec<(&xsim_export::SourceFile, String)> = design
        .sources
        .iter()
        .filter(|s| !s.lang.is_vhdl())
        .filter_map(|s| std::fs::read_to_string(&s.path).ok().map(|t| (s, t)))
        .collect();
    let tops = design.top_modules();
    let esc = |p: &str| p.replace('\\', "/").replace('"', "");
    let rel = crate::relative_path(out, export);
    let key = if export.is_dir() {
        "export_dir"
    } else {
        "filelist"
    };
    let elf = opts.elf.clone().unwrap_or_else(|| "fw/app.elf".to_string());
    let base = opts.base.unwrap_or(0x43C0_0000);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    let project = |name: &str| {
        crate::generate_project(
            &[],
            "tb/cosim_top.sv",
            &elf,
            opts.elf1.as_deref(),
            name,
            base,
        )
    };

    let own_tb = texts
        .iter()
        .find(|(_, t)| t.contains("rp_open") || t.contains("rp_next"));
    let (top, driven, toml) = if let Some((_, t)) = own_tb {
        let names = module_names(t);
        let top = tops
            .iter()
            .find(|n| names.contains(n))
            .cloned()
            .or_else(|| names.first().cloned())
            .ok_or_else(|| ApiError::invalid_argument("the testbench file declares no module"))?;
        let text = project(&top)
            .replace("files = [\n    \"tb/cosim_top.sv\",\n]\n", "")
            .replace("top = \"cosim_top\"", &format!("top = \"{top}\""));
        (top, None, text)
    } else {
        // The device under test: named, else a top of the export, else any module with an
        // AXI4-Lite slave interface the generator understands, in compile order.
        let mut candidates: Vec<String> = match &opts.module {
            Some(m) => vec![m.clone()],
            None => tops.clone(),
        };
        if opts.module.is_none() {
            for (s, t) in &texts {
                let glbl = s
                    .path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("glbl.v"));
                if glbl {
                    continue;
                }
                for n in module_names(t) {
                    if !candidates.contains(&n) {
                        candidates.push(n);
                    }
                }
            }
        }
        let mut found = None;
        let mut last_err = String::new();
        'search: for name in candidates.iter().filter(|n| *n != "glbl") {
            for (s, t) in &texts {
                if !module_names(t).contains(name) {
                    continue;
                }
                match crate::parse_module(t, Some(name))
                    .and_then(|m| crate::detect_axi_lite(&m).map(|axi| (m, axi)))
                {
                    Ok((m, axi)) => {
                        found = Some((name.clone(), m, axi));
                        break 'search;
                    }
                    Err(e) => last_err = format!("{}: module `{name}`: {e}", s.path.display()),
                }
                break;
            }
        }
        let Some((name, m, axi)) = found else {
            return Err(ApiError::unsupported(format!(
                "the export has no testbench that calls the ZynqSim `rp_*` DPI functions and no \
                 AXI4-Lite slave module a testbench can be generated for{}; write a testbench \
                 (see docs/TEST_YOUR_DESIGN.md) and set `[rtl] files`/`top`",
                if last_err.is_empty() {
                    String::new()
                } else {
                    format!(" ({last_err})")
                }
            )));
        };
        if let Err(e) = crate::detect_axi_master(&m) {
            warnings.push(format!("module `{name}`: AXI master not bridged: {e}"));
        }
        for (i, n) in crate::detect_irqs(&m, &axi).iter().enumerate() {
            warnings.push(format!(
                "interrupt output `{n}` is forwarded on IRQ_F2P[{i}]"
            ));
        }
        files.push((
            "tb/cosim_top.sv".to_string(),
            crate::generate_testbench(&m, &axi).into_bytes(),
        ));
        ("cosim_top".to_string(), Some(name.clone()), project(&name))
    };
    // Reference the export from `[rtl]` and say what generated the file.
    let toml = toml
        .replace("[rtl]\n", &format!("[rtl]\n{key} = \"{}\"\n", esc(&rel)))
        .replace("`zynqsim init --ip`", "`zynqsim init --export-sim`");
    files.insert(0, ("project.toml".to_string(), toml.clone().into_bytes()));
    files.push((
        "export-report.md".to_string(),
        report(&design, export, &top, driven.as_deref(), &warnings).into_bytes(),
    ));
    Ok(ExportInit {
        files,
        project_toml: toml,
        design,
        top,
        driven_module: driven,
        elf,
        warnings,
    })
}

fn report(
    d: &ExportDesign,
    export: &Path,
    top: &str,
    driven: Option<&str>,
    warnings: &[String],
) -> String {
    use std::fmt::Write as _;
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    let mut s = String::from("# Simulation export report\n\n");
    let _ = writeln!(s, "Export: `{}`  ", slash(export));
    let _ = writeln!(s, "Top unit: `{top}`  ");
    match driven {
        Some(m) => {
            let _ = writeln!(
                s,
                "Testbench: generated (`tb/cosim_top.sv`), drives module `{m}`"
            );
        }
        None => {
            let _ = writeln!(
                s,
                "Testbench: the export's own (it calls the `rp_*` DPI functions)"
            );
        }
    }
    let _ = writeln!(s, "\n## Sources, in compile order\n");
    let _ = writeln!(
        s,
        "| File | Language | Library | Listed at |\n|---|---|---|---|"
    );
    for f in &d.sources {
        let _ = writeln!(
            s,
            "| `{}` | {} | `{}` | {} |",
            slash(&f.path),
            f.lang.prj_word(),
            f.library,
            f.origin
        );
    }
    if d.glbl_missing {
        let _ = writeln!(
            s,
            "\n`glbl.v` is named by the export but is not on disk: Vivado's copy is used."
        );
    }
    let join = |v: &[String]| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.iter()
                .map(|x| format!("`{x}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let _ = writeln!(s, "\n## Elaboration\n");
    let _ = writeln!(s, "* libraries searched (`-L`): {}", join(&d.link_libs));
    let _ = writeln!(s, "* top units of the export: {}", join(&d.tops));
    let _ = writeln!(s, "* defines: {}", join(&d.defines));
    let inc: Vec<String> = d.include_dirs.iter().map(|p| slash(p)).collect();
    let _ = writeln!(s, "* include directories: {}", join(&inc));
    let _ = writeln!(s, "\n## Not supported, ignored or chosen\n");
    if warnings.is_empty() {
        let _ = writeln!(s, "Nothing was reported.");
    } else {
        for w in warnings {
            let _ = writeln!(s, "* {w}");
        }
    }
    let _ = writeln!(
        s,
        "\nThe export is referenced from `project.toml`, not copied: keep it where it is. \
         This report says what was read and what will be built, not that the design is correct."
    );
    s
}
