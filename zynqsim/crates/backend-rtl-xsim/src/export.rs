//! Turns a read Vivado simulation export into the `xvlog` / `xvhdl` / `xelab` invocations that
//! build it, the way Vivado's own scripts do (project-file compilation, one library per `.prj`
//! entry, `-L unisims_ver` and friends, `glbl` as a second top).
//!
//! Pure functions: nothing here runs a tool, so the command lines are unit-testable.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use xsim_export::{ExportDesign, Lang};

/// Libraries that ship with Vivado: a mapping from an exported `xsim.ini` to another machine's
/// install is never passed on, Vivado's own `xsim.ini` provides them.
const VIVADO_LIBS: &[&str] = &[
    "unisim",
    "unisims_ver",
    "unimacro",
    "unimacro_ver",
    "simprim",
    "simprims_ver",
    "secureip",
    "xpm",
    "uvm",
    "xilinx_vip",
    "axi_vip_v1_1_12",
    "xil_defaultlib",
];

/// What to run for one export build. All paths use forward slashes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    /// Content of the Verilog/SystemVerilog project file, if any file is Verilog.
    pub vlog_prj: Option<String>,
    /// Content of the VHDL project file, if any file is VHDL.
    pub vhdl_prj: Option<String>,
    /// Arguments of `xvlog` (without `-prj FILE`).
    pub xvlog_opts: String,
    /// Arguments of `xvhdl` (without `-prj FILE`).
    pub xvhdl_opts: String,
    /// Arguments of `xelab` up to and including the top units (no DPI, no snapshot).
    pub xelab_args: String,
    /// The elaborated units, `LIB.top` and `LIB.glbl`.
    pub units: Vec<String>,
    /// Every file the build reads (sources and files in include directories), for the cache key.
    pub inputs: Vec<PathBuf>,
    /// Vivado's `glbl.v` was added because nothing in the design defines it.
    pub glbl_from_vivado: bool,
}

fn fwd(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// Whether the text of `file` defines `module <name>` (or `entity <name>` for VHDL).
fn defines_unit(file: &Path, name: &str) -> bool {
    let Ok(text) = fs::read_to_string(file) else {
        return false;
    };
    for kw in ["module", "entity"] {
        let mut rest = text.as_str();
        while let Some(i) = rest.find(kw) {
            let before_ok = rest[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'));
            let after = &rest[i + kw.len()..];
            let trimmed = after.trim_start();
            if before_ok && trimmed.len() < after.len() && trimmed.starts_with(name) {
                let next = trimmed[name.len()..].chars().next();
                if next.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')) {
                    return true;
                }
            }
            rest = &rest[i + kw.len()..];
        }
    }
    false
}

/// Plans the build of `design` plus the `user` sources (testbench, extra files) with top unit
/// `top` (bare module name or `lib.module`). `defines` are the project's, applied to every file.
pub fn plan(
    design: &ExportDesign,
    user: &[PathBuf],
    top: &str,
    defines: &[String],
    vivado_root: &Path,
) -> ExportPlan {
    let default_lib = design.default_library();
    let mut vlog = String::new();
    let mut vhdl = String::new();
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<(PathBuf, String)> = Vec::new(); // (path, library) for unit lookup

    let opts = |inc: &[PathBuf], defs: &[String]| -> String {
        let mut s = String::new();
        for i in inc {
            let _ = write!(s, " -i \"{}\"", fwd(i));
        }
        for d in defs {
            let _ = write!(s, " -d \"{d}\"");
        }
        s
    };
    let mut line = |lang: Lang, lib: &str, o: &str, path: &Path| {
        // Include directories and defines are Verilog concepts.
        let (target, o) = if lang.is_vhdl() {
            (&mut vhdl, "")
        } else {
            (&mut vlog, o)
        };
        let _ = writeln!(target, "{} {lib}{o} \"{}\"", lang.prj_word(), fwd(path));
    };

    for s in &design.sources {
        let mut defs: Vec<String> = defines.to_vec();
        defs.extend(design.defines_for(s));
        let inc = design.includes_for(s);
        line(s.lang, &s.library, &opts(&inc, &defs), &s.path);
        inputs.push(s.path.clone());
        files.push((s.path.clone(), s.library.clone()));
    }
    // The user's files: Verilog as SystemVerilog, as the non-export build always did.
    let mut user_defs: Vec<String> = defines.to_vec();
    user_defs.extend(design.defines.iter().cloned());
    let user_inc: Vec<PathBuf> = design.include_dirs.clone();
    for p in user {
        let lang = match Lang::from_path(p) {
            Some(l) if l.is_vhdl() => l,
            _ => Lang::SystemVerilog,
        };
        line(lang, &default_lib, &opts(&user_inc, &user_defs), p);
        inputs.push(p.clone());
        files.push((p.clone(), default_lib.clone()));
    }

    // glbl: UNISIM primitives need it. A netlist may carry its own; otherwise Vivado's copy.
    let has_glbl = files.iter().any(|(p, _)| defines_unit(p, "glbl"));
    let wants_glbl = design.glbl_missing
        || design
            .tops
            .iter()
            .any(|t| t.rsplit('.').next().is_some_and(|m| m.eq_ignore_ascii_case("glbl")))
        || design.link_libs.iter().any(|l| l.starts_with("unisims_ver"));
    let mut glbl_from_vivado = false;
    let mut glbl_lib = files
        .iter()
        .find(|(p, _)| defines_unit(p, "glbl"))
        .map(|(_, l)| l.clone());
    if !has_glbl && wants_glbl {
        let g = vivado_root.join("data/verilog/src/glbl.v");
        let _ = writeln!(vlog, "verilog {default_lib} \"{}\"", fwd(&g));
        inputs.push(g);
        glbl_from_vivado = true;
        glbl_lib = Some(default_lib.clone());
    }

    // The top unit's library: where a file defines it, else the default.
    let top_unit = if top.contains('.') {
        top.to_string()
    } else {
        let lib = files
            .iter()
            .find(|(p, _)| defines_unit(p, top))
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| default_lib.clone());
        format!("{lib}.{top}")
    };
    let mut units = vec![top_unit];
    if let Some(l) = glbl_lib {
        units.push(format!("{l}.glbl"));
    }

    // -L: the libraries compiled here, then the export's own.
    let mut libs: Vec<String> = design.libraries();
    if !libs.contains(&default_lib) {
        libs.push(default_lib.clone());
    }
    for l in &design.link_libs {
        if !libs.contains(l) {
            libs.push(l.clone());
        }
    }
    let mut xelab = String::from("-debug typical");
    let _ = write!(
        xelab,
        " -timescale {}",
        design.timescale.as_deref().unwrap_or("1ns/1ps")
    );
    if design.relax {
        xelab.push_str(" --relax");
    }
    for l in &libs {
        // A library that is not one of Vivado's own and has a directory in the export's
        // `xsim.ini` (compiled IP libraries) is given its directory.
        let mapped = design
            .lib_maps
            .iter()
            .find(|(n, p)| n == l && !VIVADO_LIBS.contains(&l.as_str()) && p.is_dir());
        match mapped {
            Some((n, p)) => {
                let _ = write!(xelab, " -L \"{n}={}\"", fwd(p));
            }
            None => {
                let _ = write!(xelab, " -L {l}");
            }
        }
    }
    for g in &design.generics {
        let _ = write!(xelab, " -generic_top \"{g}\"");
    }
    for u in &units {
        let _ = write!(xelab, " {u}");
    }

    // Header files next to the includes take part in the cache key.
    let mut dirs: Vec<PathBuf> = design.include_dirs.clone();
    dirs.extend(design.sources.iter().flat_map(|s| s.includes.clone()));
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        if let Ok(rd) = fs::read_dir(&d) {
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            v.sort();
            inputs.extend(v.into_iter().take(2000));
        }
    }

    let relax = if design.relax { "--relax" } else { "" };
    ExportPlan {
        vlog_prj: (!vlog.is_empty()).then_some(vlog),
        vhdl_prj: (!vhdl.is_empty()).then_some(vhdl),
        xvlog_opts: relax.to_string(),
        xvhdl_opts: relax.to_string(),
        xelab_args: xelab,
        units,
        inputs,
        glbl_from_vivado,
    }
}

impl ExportPlan {
    /// The batch steps (after `call settings64.bat` and `cd`): compile, then elaborate with the
    /// DPI library. Exit codes: 1 compile, 2 elaborate.
    pub fn batch_steps(&self, dpi_dir: &str, dpi_name: &str) -> String {
        let mut s = String::new();
        if self.vlog_prj.is_some() {
            let _ = write!(
                s,
                "call xvlog {} -prj zs_vlog.prj || exit /b 1\r\n",
                self.xvlog_opts
            );
        }
        if self.vhdl_prj.is_some() {
            let _ = write!(
                s,
                "call xvhdl {} -prj zs_vhdl.prj || exit /b 1\r\n",
                self.xvhdl_opts
            );
        }
        let _ = write!(
            s,
            "call xelab {} -sv_root \"{dpi_dir}\" -sv_lib {dpi_name} -s zs_snap || exit /b 2\r\n",
            self.xelab_args
        );
        s
    }

    /// Writes the project files into `dir`.
    pub fn write_prj(&self, dir: &Path) -> std::io::Result<()> {
        // `nosort` keeps the order of the lists, as Vivado's own files do.
        if let Some(t) = &self.vlog_prj {
            fs::write(dir.join("zs_vlog.prj"), format!("{t}\nnosort\n"))?;
        }
        if let Some(t) = &self.vhdl_prj {
            fs::write(dir.join("zs_vhdl.prj"), format!("{t}\nnosort\n"))?;
        }
        Ok(())
    }

    /// Everything that changes the compiled snapshot, as bytes for the cache key (project files,
    /// elaboration arguments, and the content of every input file).
    pub fn fingerprint(&self) -> std::io::Result<Vec<u8>> {
        let mut b = Vec::new();
        b.extend(self.vlog_prj.clone().unwrap_or_default().into_bytes());
        b.push(0);
        b.extend(self.vhdl_prj.clone().unwrap_or_default().into_bytes());
        b.push(0);
        b.extend(self.xvlog_opts.as_bytes());
        b.push(0);
        b.extend(self.xelab_args.as_bytes());
        b.push(0);
        for p in &self.inputs {
            b.extend(fwd(p).as_bytes());
            b.push(0);
            b.extend(fs::read(p).unwrap_or_default());
            b.push(0);
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xsim_export::{Origin, SourceFile};

    fn src(path: &str, lang: Lang, lib: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            lang,
            library: lib.to_string(),
            includes: Vec::new(),
            defines: Vec::new(),
            origin: Origin {
                file: PathBuf::from("x.prj"),
                line: 1,
            },
        }
    }

    fn design() -> ExportDesign {
        ExportDesign {
            sources: vec![
                src("C:/d/net.v", Lang::Verilog, "xil_defaultlib"),
                src("C:/d/pkg.sv", Lang::SystemVerilog, "xil_defaultlib"),
                src("C:/d/ip.vhd", Lang::Vhdl, "blk_mem_gen_v8_4_5"),
            ],
            link_libs: vec![
                "xil_defaultlib".into(),
                "unisims_ver".into(),
                "secureip".into(),
            ],
            tops: vec!["xil_defaultlib.tb".into(), "xil_defaultlib.glbl".into()],
            relax: true,
            ..ExportDesign::default()
        }
    }

    #[test]
    fn projects_keep_order_language_library_and_options() {
        let mut d = design();
        d.sources[0].includes = vec![PathBuf::from("C:/d/inc")];
        d.sources[0].defines = vec!["W=8".into()];
        let p = plan(
            &d,
            &[PathBuf::from("C:/t/tb.sv")],
            "cosim_top",
            &["SIM".to_string()],
            Path::new("C:/Xilinx/Vivado/2022.1"),
        );
        let vlog = p.vlog_prj.unwrap();
        let lines: Vec<&str> = vlog.lines().collect();
        assert_eq!(
            lines[0],
            "verilog xil_defaultlib -i \"C:/d/inc\" -d \"SIM\" -d \"W=8\" \"C:/d/net.v\""
        );
        assert_eq!(lines[1], "sv xil_defaultlib -d \"SIM\" \"C:/d/pkg.sv\"");
        // The user's testbench follows, as SystemVerilog, in the default library.
        assert_eq!(lines[2], "sv xil_defaultlib -d \"SIM\" \"C:/t/tb.sv\"");
        // Vivado's glbl.v is added: nothing defines it.
        assert_eq!(
            lines[3],
            "verilog xil_defaultlib \"C:/Xilinx/Vivado/2022.1/data/verilog/src/glbl.v\""
        );
        assert!(p.glbl_from_vivado);
        assert_eq!(
            p.vhdl_prj.unwrap(),
            "vhdl blk_mem_gen_v8_4_5 \"C:/d/ip.vhd\"\n"
        );
    }

    #[test]
    fn xelab_gets_libraries_unisim_and_both_tops() {
        let p = plan(
            &design(),
            &[PathBuf::from("C:/t/tb.sv")],
            "cosim_top",
            &[],
            Path::new("C:/V"),
        );
        assert_eq!(
            p.xelab_args,
            "-debug typical -timescale 1ns/1ps --relax -L xil_defaultlib -L blk_mem_gen_v8_4_5 \
             -L unisims_ver -L secureip xil_defaultlib.cosim_top xil_defaultlib.glbl"
        );
        let steps = p.batch_steps("C:/w/dpi", "zynqsim_dpi");
        assert!(steps.contains("call xvlog --relax -prj zs_vlog.prj || exit /b 1"));
        assert!(steps.contains("call xvhdl --relax -prj zs_vhdl.prj || exit /b 1"));
        assert!(steps.contains("-sv_root \"C:/w/dpi\" -sv_lib zynqsim_dpi -s zs_snap"));
        assert!(steps.find("xvlog").unwrap() < steps.find("xelab").unwrap());
    }

    #[test]
    fn a_dotted_top_is_taken_as_written_and_generics_are_quoted() {
        let mut d = design();
        d.generics = vec!["W=8".into()];
        d.timescale = Some("1ns/1ns".into());
        let p = plan(&d, &[], "work.my_tb", &[], Path::new("C:/V"));
        assert!(p.xelab_args.contains("-generic_top \"W=8\""));
        assert!(p.xelab_args.contains("-timescale 1ns/1ns"));
        assert_eq!(p.units[0], "work.my_tb");
    }

    #[test]
    fn a_netlist_that_defines_glbl_is_not_given_a_second_one() {
        let dir = std::env::temp_dir().join(format!("zs-exp-plan-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let net = dir.join("net.v");
        fs::write(&net, "module my_ip(input a);\nendmodule\nmodule glbl();\nendmodule\n").unwrap();
        let mut d = design();
        d.sources = vec![SourceFile {
            path: net.clone(),
            ..src("x", Lang::Verilog, "xil_defaultlib")
        }];
        let p = plan(&d, &[], "tb", &[], Path::new("C:/V"));
        assert!(!p.glbl_from_vivado);
        assert!(!p.vlog_prj.unwrap().contains("glbl.v"));
        assert_eq!(p.units, ["xil_defaultlib.tb", "xil_defaultlib.glbl"]);
        // The top is looked up where it is defined.
        let p2 = plan(&d, &[], "my_ip", &[], Path::new("C:/V"));
        assert_eq!(p2.units[0], "xil_defaultlib.my_ip");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_compiled_ip_library_with_a_directory_is_mapped_vivado_libraries_are_not() {
        let dir = std::env::temp_dir().join(format!("zs-exp-map-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut d = design();
        d.lib_maps = vec![
            ("blk_mem_gen_v8_4_5".into(), dir.clone()),
            ("unisims_ver".into(), dir.clone()),
        ];
        let p = plan(&d, &[], "tb", &[], Path::new("C:/V"));
        assert!(
            p.xelab_args
                .contains(&format!("-L \"blk_mem_gen_v8_4_5={}\"", fwd(&dir)))
        );
        assert!(p.xelab_args.contains(" -L unisims_ver"));
        let _ = fs::remove_dir_all(&dir);
    }
}
