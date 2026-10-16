//! The Verilator adapter against a real Verilator build and run of the prototype's
//! `my_math_ip`, with no PS: the test plays the PS by calling the `PlTarget` API.
//!
//! Needs Verilator (`verilator_bin.exe`, found by `backend_rtl_verilator::tools::find`:
//! `ZYNQSIM_VERILATOR`, `VERILATOR_ROOT`, `PATH`, `<temp>/zs-tools/verilator/...`), plus
//! `make`, a C++ compiler and `sh` on the path. When Verilator is absent the tests print
//! `SKIPPED:` and return.

use backend_rtl_verilator::{VerilatorConfig, VerilatorError, VerilatorPl};
use pl_api::{BusResult, PlTarget, PsMasterPort, PsPort};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Builds the freestanding DPI library if it is not there yet.
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
                "--target-dir",
                "target/dpi",
            ])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
    lib
}

fn work(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-verilator-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn fixture(rel: &str) -> PathBuf {
    workspace().join("fixtures").join("rtl").join(rel)
}

fn config(
    tag: &str,
    files: &[PathBuf],
    top: &str,
    cache: Option<&Path>,
) -> Option<VerilatorConfig> {
    let tools = backend_rtl_verilator::tools::find()?;
    Some(VerilatorConfig {
        verilator: tools.verilator,
        root: tools.root,
        tool_dirs: tools.tool_dirs,
        sources: files.to_vec(),
        top: top.to_string(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work(tag),
        connect_timeout: Duration::from_secs(120),
        cache_dir: cache.map(Path::to_path_buf),
    })
}

/// A PS with no memory: any PL master access is an error.
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

fn word(pl: &mut VerilatorPl, addr: u64, t: u64) -> u32 {
    let mut b = [0u8; 4];
    pl.read(0, addr, &mut b, t, &mut NoPs).expect("read");
    u32::from_le_bytes(b)
}

#[test]
fn rtl_math_ip_answers_ps_accesses_through_verilator() {
    let cache = std::env::temp_dir().join("zynqsim-verilator-test-cache");
    let Some(cfg) = config(
        "math",
        &[fixture("math/my_math_ip.v"), fixture("math/cosim_top.sv")],
        "cosim_top",
        Some(&cache),
    ) else {
        eprintln!(
            "SKIPPED: Verilator not found (set ZYNQSIM_VERILATOR or unpack to <temp>/zs-tools)"
        );
        return;
    };
    let started = std::time::Instant::now();
    let mut pl = VerilatorPl::start(&cfg).unwrap_or_else(|e| panic!("start failed: {e}"));
    let first_start = started.elapsed();

    let vectors = [
        0u32,
        1,
        0x15,
        0x1234_5678,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
    ];
    let mut t = 1_000;
    for v in vectors {
        pl.write(0, 0x43C0_0000, &v.to_le_bytes(), t, &mut NoPs)
            .unwrap();
        t += 100;
        assert_eq!(word(&mut pl, 0x43C0_0000, t), v, "echo of {v:#x}");
        t += 100;
        assert_eq!(
            word(&mut pl, 0x43C0_0004, t),
            v.wrapping_mul(2),
            "2 * {v:#x}"
        );
        t += 100;
    }
    let s = pl.stats();
    assert_eq!((s.ps_writes, s.ps_reads), (7, 14));

    // Lock-step advance returns once the design has answered the sync.
    pl.advance(t + 10_000, &mut NoPs);
    assert!(!pl.finished());
    assert!(pl.stats().syncs >= 1);
    assert!(cfg.work_dir.join("build.log").is_file());
    drop(pl);

    // The second start reuses the cached executable: nothing is translated again.
    let cfg2 = VerilatorConfig {
        work_dir: work("math-again"),
        ..cfg.clone()
    };
    let started = std::time::Instant::now();
    let mut pl = VerilatorPl::start(&cfg2).expect("cached start");
    let cached_start = started.elapsed();
    pl.write(0, 0x43C0_0000, &21u32.to_le_bytes(), 1_000, &mut NoPs)
        .unwrap();
    assert_eq!(
        word(&mut pl, 0x43C0_0004, 2_000),
        42,
        "the cached model computes"
    );
    let build = std::fs::read_to_string(cfg2.work_dir.join("build.log")).unwrap();
    assert!(!build.contains("%Error"), "{build}");
    eprintln!("verilator start: first {first_start:?}, cached {cached_start:?}");
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn unsupported_constructs_are_reported_with_file_line_and_text() {
    let Some(base) = config("probe", &[], "top", None) else {
        eprintln!("SKIPPED: Verilator not found");
        return;
    };
    let dir = work("bad");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad3.sv");
    std::fs::write(
        &bad,
        "module top;\n  interface bus_if;\n    logic v;\n  endinterface\n  bus_if b();\n  initial begin\n    $display(\"unused %0d\", b.v);\n    $finish;\n  end\nendmodule\n",
    )
    .unwrap();
    let cfg = VerilatorConfig {
        sources: vec![bad.clone()],
        work_dir: dir.join("w"),
        ..base
    };
    match VerilatorPl::start(&cfg) {
        Ok(_) => panic!("an interface inside a module must not build"),
        Err(VerilatorError::Build {
            step, diagnostics, ..
        }) => {
            assert!(step.starts_with("verilator"), "{step}");
            let d = diagnostics
                .iter()
                .find(|d| d.code.as_deref() == Some("UNSUPPORTED"))
                .unwrap_or_else(|| panic!("no UNSUPPORTED message in {diagnostics:?}"));
            assert_eq!(d.file_name(), Some("bad3.sv"));
            // Verilator points at the instance, the first place the declaration is used.
            assert_eq!(d.line, Some(5));
            assert_eq!(d.construct, "bus_if b();");
        }
        Err(other) => panic!("wrong error: {other}"),
    }
}
