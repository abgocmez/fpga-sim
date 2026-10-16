//! Vivado simulation exports through the adapter: the real Vivado 2022.1 `export_simulation`
//! of `my_math_ip` (RTL) and the post-synthesis functional netlist (`write_verilog -mode
//! funcsim`, UNISIM LUT/FDRE cells, `-L unisims_ver`, `glbl`) answer the same bus accesses.
//!
//! Needs Vivado (XSim). When absent the tests print `SKIPPED:` and return.

use backend_rtl_xsim::{ExportDesign, XsimConfig, XsimPl};
use pl_api::{BusResult, PlTarget, PsMasterPort, PsPort};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn find_vivado() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_VIVADO") {
        return Some(PathBuf::from(p));
    }
    let root = PathBuf::from(r"C:\Xilinx\Vivado");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("settings64.bat").is_file())
        .collect();
    versions.sort();
    versions.pop()
}

fn dpi_lib() -> PathBuf {
    let lib = workspace()
        .join("target")
        .join("dpi")
        .join("release")
        .join("libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
            ])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
    lib
}

struct NoPs;

impl PsPort for NoPs {
    fn mem_read(&mut self, _: PsMasterPort, _: u64, _: &mut [u8]) -> BusResult<()> {
        Err(pl_api::BusError::Decode)
    }
    fn mem_write(&mut self, _: PsMasterPort, _: u64, _: &[u8]) -> BusResult<()> {
        Err(pl_api::BusError::Decode)
    }
    fn irq(&mut self, _: u32, _: bool, _: u64) -> BusResult<()> {
        Ok(())
    }
}

/// Builds the export under `dir` (relative to `fixtures/export`), writes the 7 vectors and
/// returns the (echo, doubled) read-backs, after checking the build used what it should.
fn run_export(tag: &str, dir: &str, flavour: &str) -> Option<Vec<(u32, u32)>> {
    let vivado = find_vivado()?;
    let root = workspace().join("fixtures").join("export").join(dir);
    let design: ExportDesign = xsim_export::read(&root).expect("export is read");
    assert!(!design.has_errors(), "{:?}", design.diagnostics);
    let work = std::env::temp_dir().join(format!("zynqsim-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let cfg = XsimConfig {
        vivado_root: vivado,
        sources: Vec::new(),
        top: "cosim_top".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work.clone(),
        waves: false,
        connect_timeout: Duration::from_secs(180),
        cache_dir: None,
    };
    let mut pl =
        XsimPl::start_with(&cfg, Some(&design)).unwrap_or_else(|e| panic!("start failed: {e}"));
    let prj = std::fs::read_to_string(work.join("zs_vlog.prj")).unwrap();
    let build = std::fs::read_to_string(work.join("build.bat")).unwrap();
    eprintln!("--- zs_vlog.prj\n{prj}\n--- build.bat\n{build}");
    assert!(build.contains("-prj zs_vlog.prj"), "{build}");
    match flavour {
        "unisim" => {
            assert!(build.contains("-L unisims_ver"), "{build}");
            assert!(build.contains("xil_defaultlib.glbl"), "{build}");
        }
        "vhdl" => {
            // The VHDL file is compiled by xvhdl, the Verilog testbench by xvlog; one snapshot.
            assert!(build.contains("xvhdl"), "{build}");
            assert!(build.contains("-prj zs_vhdl.prj"), "{build}");
            let vhdl = std::fs::read_to_string(work.join("zs_vhdl.prj")).unwrap();
            assert!(vhdl.starts_with("vhdl xil_defaultlib "), "{vhdl}");
            assert!(!build.contains("glbl"), "{build}");
        }
        other => panic!("{other}"),
    }

    let vectors = [
        0u32,
        1,
        0x15,
        0x1234_5678,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
    ];
    let mut out = Vec::new();
    let mut t = 1_000;
    for v in vectors {
        pl.write(0, 0x43C0_0000, &v.to_le_bytes(), t, &mut NoPs)
            .unwrap();
        t += 100;
        let mut a = [0u8; 4];
        pl.read(0, 0x43C0_0000, &mut a, t, &mut NoPs).unwrap();
        t += 100;
        let mut b = [0u8; 4];
        pl.read(0, 0x43C0_0004, &mut b, t, &mut NoPs).unwrap();
        t += 100;
        out.push((u32::from_le_bytes(a), u32::from_le_bytes(b)));
    }
    let s = pl.stats();
    assert_eq!((s.ps_writes, s.ps_reads), (7, 14));
    Some(out)
}

#[test]
fn the_rtl_export_and_the_funcsim_netlist_give_the_same_answers() {
    let Some(rtl) = run_export("rtl", "behav/xsim", "unisim") else {
        eprintln!("SKIPPED: Vivado/XSim not found (set ZYNQSIM_VIVADO)");
        return;
    };
    let net = run_export("net", "postsynth", "unisim").expect("vivado present");
    for (v, (echo, dbl)) in [
        0u32,
        1,
        0x15,
        0x1234_5678,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
    ]
    .iter()
    .zip(&rtl)
    {
        assert_eq!(echo, v);
        assert_eq!(*dbl, v.wrapping_mul(2));
    }
    assert_eq!(
        rtl, net,
        "the post-synthesis netlist must behave like the RTL"
    );
}

/// A VHDL design unit (`xvhdl`) instantiated from the Verilog testbench: the same answers.
#[test]
fn a_vhdl_design_file_is_compiled_with_xvhdl_and_answers_like_the_verilog() {
    let Some(vhdl) = run_export("vhdl", "vhdl", "vhdl") else {
        eprintln!("SKIPPED: Vivado/XSim not found (set ZYNQSIM_VIVADO)");
        return;
    };
    for (v, (echo, dbl)) in [
        0u32,
        1,
        0x15,
        0x1234_5678,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
    ]
    .iter()
    .zip(&vhdl)
    {
        assert_eq!(echo, v);
        assert_eq!(*dbl, v.wrapping_mul(2));
    }
}

/// Include directories and defines of a `.prj` line reach `xvlog`, and the snapshot of an
/// export is cached by the content of everything it was built from (including a header).
#[test]
fn prj_includes_and_defines_reach_xvlog_and_the_build_is_cached() {
    let Some(vivado) = find_vivado() else {
        eprintln!("SKIPPED: Vivado/XSim not found (set ZYNQSIM_VIVADO)");
        return;
    };
    let root = std::env::temp_dir().join(format!("zynqsim-export-inc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("inc")).unwrap();
    let fx = workspace().join("fixtures").join("export").join("rtl");
    std::fs::copy(fx.join("my_math_ip.v"), root.join("my_math_ip.v")).unwrap();
    // The testbench only compiles when the include directory and the define arrived.
    let tb = std::fs::read_to_string(fx.join("cosim_top.sv")).unwrap();
    let guarded = format!(
        "`include \"ver.vh\"
`ifndef VER_OK
  include_dir_missing_syntax_error;
`endif
         `ifndef MAGIC
  define_missing_syntax_error;
`endif
{tb}"
    );
    std::fs::write(root.join("cosim_top.sv"), guarded).unwrap();
    std::fs::write(
        root.join("inc/ver.vh"),
        "`define VER_OK
",
    )
    .unwrap();
    std::fs::write(
        root.join("files.prj"),
        "verilog xil_defaultlib \"my_math_ip.v\"
         sv xil_defaultlib -i \"inc\" -d \"MAGIC=5\" \"cosim_top.sv\"
",
    )
    .unwrap();
    let design = xsim_export::read(&root.join("files.prj")).unwrap();
    assert!(!design.has_errors(), "{:?}", design.diagnostics);
    let cache = root.join("cache");
    let cfg = |tag: &str| XsimConfig {
        vivado_root: vivado.clone(),
        sources: Vec::new(),
        top: "cosim_top".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: root.join(tag),
        waves: false,
        connect_timeout: Duration::from_secs(180),
        cache_dir: Some(cache.clone()),
    };
    let mut first = XsimPl::start_with(&cfg("w1"), Some(&design)).unwrap_or_else(|e| panic!("{e}"));
    let mut b = [0u8; 4];
    first
        .read(0, 0x43C0_0004, &mut b, 1_000, &mut NoPs)
        .unwrap();
    drop(first);
    let log1 = std::fs::read_to_string(root.join("w1/build.log")).unwrap();
    assert!(!log1.contains("reused the cached build"), "{log1}");
    let second = XsimPl::start_with(&cfg("w2"), Some(&design)).unwrap_or_else(|e| panic!("{e}"));
    drop(second);
    let log2 = std::fs::read_to_string(root.join("w2/build.log")).unwrap();
    assert!(log2.contains("reused the cached build"), "{log2}");
    // Changing the header changes the key: it is compiled again.
    std::fs::write(
        root.join("inc/ver.vh"),
        "`define VER_OK
// changed
",
    )
    .unwrap();
    let third = XsimPl::start_with(&cfg("w3"), Some(&design)).unwrap_or_else(|e| panic!("{e}"));
    drop(third);
    let log3 = std::fs::read_to_string(root.join("w3/build.log")).unwrap();
    assert!(!log3.contains("reused the cached build"), "{log3}");
    let _ = std::fs::remove_dir_all(&root);
}
