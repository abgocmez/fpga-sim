use super::*;
use std::fs;

/// A scratch folder that is removed on drop.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("xsim-export-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/export")
        .join(rel)
}

fn names(d: &ExportDesign) -> Vec<String> {
    d.sources
        .iter()
        .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn no_env() -> Reader {
    Reader::with_env(|_| None)
}

#[test]
fn a_vivado_prj_gives_ordered_sources_libraries_includes_and_defines() {
    let t = Tmp::new("prj");
    for f in ["a.v", "b.sv", "c.vhd", "inc/x.vh", "my dir/space.v", "d.v"] {
        t.file(f, "");
    }
    t.file(
        "design.prj",
        r#"# compile verilog/system verilog design source files
verilog xil_defaultlib  -i "inc" -d "WIDTH=8" -d SIM \
"a.v" \
"my dir/space.v" \

sv xil_defaultlib --include "inc" "b.sv"   // trailing comment
vhdl xpm "c.vhd"
verilog work "sub/../d.v" -L unisims_ver

# Do not sort compile order
nosort
"#,
    );
    let d = no_env().read(&t.0.join("design.prj")).unwrap();
    assert_eq!(names(&d), ["a.v", "space.v", "b.sv", "c.vhd", "d.v"]);
    assert!(!d.has_errors(), "{:?}", d.diagnostics);
    let langs: Vec<Lang> = d.sources.iter().map(|s| s.lang).collect();
    assert_eq!(
        langs,
        [
            Lang::Verilog,
            Lang::Verilog,
            Lang::SystemVerilog,
            Lang::Vhdl,
            Lang::Verilog
        ]
    );
    let libs: Vec<&str> = d.sources.iter().map(|s| s.library.as_str()).collect();
    assert_eq!(
        libs,
        [
            "xil_defaultlib",
            "xil_defaultlib",
            "xil_defaultlib",
            "xpm",
            "work"
        ]
    );
    // Options of a line belong to the files of that line.
    assert_eq!(d.sources[0].defines, ["WIDTH=8", "SIM"]);
    assert_eq!(d.sources[1].defines, ["WIDTH=8", "SIM"]);
    assert!(d.sources[2].defines.is_empty());
    assert_eq!(d.sources[0].includes, [lex_resolve(&t.0, "inc")]);
    assert_eq!(d.sources[2].includes.len(), 1);
    // A relative `..` is folded, so the path points at the file.
    assert_eq!(d.sources[4].path, lex_resolve(&t.0, "d.v"));
    assert_eq!(d.link_libs, ["unisims_ver"]);
    assert_eq!(d.libraries(), ["xil_defaultlib", "xpm", "work"]);
}

fn lex_resolve(base: &Path, rel: &str) -> PathBuf {
    crate::lex::resolve(base, rel)
}

#[test]
fn unknown_options_and_missing_files_name_file_line_and_construct() {
    let t = Tmp::new("diag");
    t.file("a.v", "");
    let p = t.file(
        "x.prj",
        "verilog work \"a.v\"\nverilog work --frobnicate \"a.v\"\nverilog work \"gone.v\"\nbogus line\n",
    );
    let d = no_env().read(&p).unwrap();
    let at = |line: usize| d.diagnostics.iter().find(|x| x.line == line).unwrap();
    assert_eq!(at(2).construct, "--frobnicate");
    assert_eq!(at(2).severity, Severity::Warning);
    assert_eq!(
        at(3).construct.replace('\\', "/"),
        t.0.join("gone.v").display().to_string().replace('\\', "/")
    );
    assert_eq!(at(3).severity, Severity::Error);
    assert_eq!(at(4).construct, "bogus");
    assert!(d.has_errors());
    // The message form carries all three.
    let msg = at(3).to_string();
    assert!(msg.contains("x.prj:3") && msg.contains("gone.v"), "{msg}");
    // The duplicate of line 1 on line 2 is dropped.
    assert_eq!(names(&d), ["a.v"]);
}

#[test]
fn dpi_libraries_and_search_directories_are_reported_not_silently_dropped() {
    let t = Tmp::new("unsup");
    let p = t.file(
        "e.opt",
        "-L xil_defaultlib -sv_lib mylib -y libdir work.top\n",
    );
    let d = no_env().read(&p).unwrap();
    let c: Vec<&str> = d.diagnostics.iter().map(|x| x.construct.as_str()).collect();
    assert!(c.contains(&"-sv_lib mylib"), "{c:?}");
    assert!(c.contains(&"-y libdir"), "{c:?}");
    assert_eq!(d.tops, ["work.top"]);
}

#[test]
fn a_dot_f_list_has_includes_defines_comments_and_nested_lists() {
    let t = Tmp::new("dotf");
    for f in ["top.sv", "u.v", "lib/l.vhd", "rtl/k.v", "defs.svh"] {
        t.file(f, "");
    }
    t.file("more.f", "rtl/k.v   // another\n");
    let p = t.file(
        "files.f",
        "// project list\n+incdir+inc1+inc2\n+define+FAST+DEPTH=16\n-sv\n-work mylib\n\
         top.sv\n# comment\n\"u.v\" /* inline */ lib/l.vhd\ndefs.svh\n-f more.f\n-D FOO=1\n",
    );
    let d = no_env().read(&p).unwrap();
    assert_eq!(names(&d), ["top.sv", "u.v", "l.vhd", "k.v"]);
    assert_eq!(
        d.sources[1].lang,
        Lang::SystemVerilog,
        "-sv promotes plain Verilog"
    );
    assert_eq!(d.sources[2].lang, Lang::Vhdl);
    assert!(d.sources[..3].iter().all(|s| s.library == "mylib"));
    assert_eq!(d.defines, ["FAST", "DEPTH=16", "FOO=1"]);
    assert_eq!(d.include_dirs.len(), 2);
    // `inc1`/`inc2` do not exist: warnings, not errors.
    assert!(!d.has_errors(), "{:?}", d.diagnostics);
    assert!(d.diagnostics.iter().any(|x| x.construct.ends_with("inc1")));
}

#[test]
fn elab_opt_gives_libraries_tops_timescale_and_generics() {
    let t = Tmp::new("elab");
    let p = t.file(
        "elab.opt",
        "--incr --debug typical --relax --mt auto -L xil_defaultlib -L uvm -L unisims_ver \\\n\
         -L xpm=ip/xpm --snapshot tb_behav xil_defaultlib.tb xil_defaultlib.glbl \\\n\
         -timescale 1ns/1ps -generic_top \"W=8\" -log elaborate.log\n",
    );
    let d = no_env().read(&p).unwrap();
    assert_eq!(d.link_libs, ["xil_defaultlib", "uvm", "unisims_ver", "xpm"]);
    assert_eq!(d.tops, ["xil_defaultlib.tb", "xil_defaultlib.glbl"]);
    assert_eq!(d.top_modules(), ["tb"]);
    assert_eq!(d.timescale.as_deref(), Some("1ns/1ps"));
    assert_eq!(d.generics, ["W=8"]);
    assert!(d.relax);
    assert_eq!(
        d.lib_maps,
        [("xpm".to_string(), lex_resolve(&t.0, "ip/xpm"))]
    );
    assert!(d.diagnostics.is_empty(), "{:?}", d.diagnostics);
}

#[test]
fn environment_variables_expand_and_an_unknown_one_is_an_error_with_its_line() {
    let t = Tmp::new("env");
    let vivado = t.file("viv/data/verilog/src/glbl.v", "module glbl; endmodule");
    let p = t.file(
        "x.prj",
        "verilog work \"$XILINX_VIVADO/data/verilog/src/glbl.v\"\n\
         verilog work \"%NOPE%/x.v\"\n",
    );
    let root = t.0.join("viv").display().to_string().replace('\\', "/");
    let r = Reader::with_env(move |n| (n == "XILINX_VIVADO").then(|| root.clone()));
    let d = r.read(&p).unwrap();
    assert_eq!(d.sources.len(), 1);
    assert_eq!(
        d.sources[0].path.display().to_string().replace('\\', "/"),
        vivado.display().to_string().replace('\\', "/")
    );
    let e = d
        .diagnostics
        .iter()
        .find(|x| x.construct == "$NOPE")
        .unwrap();
    assert_eq!(e.line, 2);
    assert_eq!(e.severity, Severity::Error);
}

#[test]
fn a_missing_glbl_is_flagged_for_the_vivado_copy_not_an_error() {
    let t = Tmp::new("glbl");
    t.file("a.v", "");
    let p = t.file("x.prj", "verilog work \"a.v\"\nverilog work \"glbl.v\"\n");
    let d = no_env().read(&p).unwrap();
    assert!(d.glbl_missing);
    assert!(!d.has_errors());
    assert_eq!(names(&d), ["a.v"]);
}

#[test]
fn a_script_is_followed_to_its_lists_and_expands_its_option_variables() {
    let t = Tmp::new("script");
    t.file("one.v", "");
    t.file("two.vhd", "");
    t.file("v.prj", "verilog xil_defaultlib \"one.v\"\n");
    t.file("h.prj", "vhdl xil_defaultlib \"two.vhd\"\n");
    t.file(
        "run.sh",
        "#!/bin/bash -f\nxvlog_opts=\"--incr --relax -L uvm\"\ncompile()\n{\n  xvlog $xvlog_opts -prj v.prj 2>&1 | tee compile.log\n  xvhdl --relax -prj h.prj\n}\n\
         elaborate()\n{\n  xelab -L xil_defaultlib -L unisims_ver --snapshot s xil_defaultlib.top xil_defaultlib.glbl -log e.log\n}\n",
    );
    let d = no_env().read(&t.0).unwrap();
    assert_eq!(names(&d), ["one.v", "two.vhd"]);
    assert_eq!(d.link_libs, ["uvm", "xil_defaultlib", "unisims_ver"]);
    assert_eq!(d.tops, ["xil_defaultlib.top", "xil_defaultlib.glbl"]);
    assert!(d.relax);
}

#[test]
fn a_nest_of_lists_that_include_each_other_terminates() {
    let t = Tmp::new("loop");
    t.file("a.v", "");
    t.file("a.f", "a.v\n-f b.f\n");
    t.file("b.f", "-f a.f\n");
    let d = no_env().read(&t.0.join("a.f")).unwrap();
    assert_eq!(names(&d), ["a.v"]);
}

#[test]
fn the_real_vivado_2022_1_behavioural_export_is_read() {
    let d = no_env().read(&fixture("behav/xsim")).unwrap();
    assert!(!d.has_errors(), "{:?}", d.diagnostics);
    assert_eq!(names(&d), ["my_math_ip.v", "cosim_top.sv", "glbl.v"]);
    assert_eq!(d.sources[0].lang, Lang::Verilog);
    assert_eq!(d.sources[1].lang, Lang::SystemVerilog);
    assert!(d.sources.iter().all(|s| s.library == "xil_defaultlib"));
    assert!(d.has_glbl_source());
    assert_eq!(
        d.link_libs,
        [
            "uvm",
            "xil_defaultlib",
            "unisims_ver",
            "unimacro_ver",
            "secureip"
        ]
    );
    assert_eq!(d.tops, ["xil_defaultlib.cosim_top", "xil_defaultlib.glbl"]);
    assert_eq!(d.top_modules(), ["cosim_top"]);
    assert!(d.relax);
    assert!(
        d.lib_maps
            .iter()
            .any(|(n, p)| n == "xil_defaultlib" && p.ends_with("xsim.dir/xil_defaultlib"))
    );
}

#[test]
fn the_real_post_synthesis_export_names_the_netlist_and_unisim() {
    let d = no_env().read(&fixture("postsynth")).unwrap();
    assert!(!d.has_errors(), "{:?}", d.diagnostics);
    assert_eq!(names(&d), ["cosim_top_func_synth.v", "cosim_top.sv"]);
    assert!(d.link_libs.contains(&"unisims_ver".to_string()));
    assert!(d.tops.contains(&"xil_defaultlib.glbl".to_string()));
    // The prj names no glbl.v: the consumer supplies Vivado's.
    assert!(!d.has_glbl_source());
}

#[test]
fn a_folder_with_two_flows_asks_for_one() {
    let e = no_env().read(&fixture(".")).unwrap_err();
    assert!(e.contains("several exports"), "{e}");
    assert!(e.contains("postsynth") && e.contains("behav"), "{e}");
}

#[test]
fn nonsense_inputs_are_explained() {
    let t = Tmp::new("bad");
    assert!(
        no_env()
            .read(&t.0.join("nope.prj"))
            .unwrap_err()
            .contains("does not exist")
    );
    assert!(
        no_env()
            .read(&t.0)
            .unwrap_err()
            .contains("export_simulation")
    );
}

#[test]
fn a_single_netlist_file_is_a_one_file_export_with_the_unisim_libraries() {
    let t = Tmp::new("one");
    let v = t.file("net.v", "module m; endmodule\n");
    let d = no_env().read(&v).unwrap();
    assert_eq!(names(&d), ["net.v"]);
    assert_eq!(d.sources[0].library, "xil_defaultlib");
    assert_eq!(d.link_libs, ["unisims_ver", "unimacro_ver", "secureip"]);
    assert!(!d.has_errors());
}
