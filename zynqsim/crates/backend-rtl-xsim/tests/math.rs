//! The RTL adapter against a real XSim run of the prototype's `my_math_ip`,
//! with no PS at all: the test plays the PS by calling the `PlTarget` API.
//!
//! Needs Vivado (XSim). When absent the tests print `SKIPPED:` and return.

use backend_rtl_xsim::{XsimConfig, XsimPl};
use pl_api::{BusResult, PlTarget, PsMasterPort, PsPort};
use std::path::{Path, PathBuf};
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
            ])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
    lib
}

fn work(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-xsim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn fixture(rel: &str) -> PathBuf {
    workspace().join("fixtures").join("rtl").join(rel)
}

fn config(tag: &str, files: &[&str], top: &str) -> Option<XsimConfig> {
    let vivado = find_vivado()?;
    Some(XsimConfig {
        vivado_root: vivado,
        sources: files.iter().map(|f| fixture(f)).collect(),
        top: top.to_string(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work(tag),
        waves: false,
        connect_timeout: Duration::from_secs(180),
        cache_dir: None,
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

fn word(pl: &mut XsimPl, addr: u64, t: u64) -> u32 {
    let mut b = [0u8; 4];
    pl.read(0, addr, &mut b, t, &mut NoPs).expect("read");
    u32::from_le_bytes(b)
}

#[test]
fn rtl_math_ip_answers_ps_accesses_through_xsim() {
    let Some(cfg) = config(
        "math",
        &["math/my_math_ip.v", "math/cosim_top.sv"],
        "cosim_top",
    ) else {
        eprintln!("SKIPPED: Vivado/XSim not found (set ZYNQSIM_VIVADO)");
        return;
    };
    let mut pl = XsimPl::start(&cfg).unwrap_or_else(|e| panic!("start failed: {e}"));

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
    assert!(cfg.work_dir.join("build.log").is_file());
}

#[test]
fn build_errors_are_reported_with_the_compiler_output() {
    let Some(vivado) = find_vivado() else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let dir = work("bad");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.v");
    std::fs::write(&bad, "module top; this is not verilog; endmodule\n").unwrap();
    let cfg = XsimConfig {
        vivado_root: vivado,
        sources: vec![bad],
        top: "top".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: dir.join("w"),
        waves: false,
        connect_timeout: Duration::from_secs(5),
        cache_dir: None,
    };
    match XsimPl::start(&cfg) {
        Err(backend_rtl_xsim::XsimError::Build { step, log }) => {
            assert!(step.contains("xvlog"), "{step}");
            assert!(log.contains("bad.v"), "{log}");
        }
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("must not start"),
    }
}

#[test]
fn missing_files_are_reported_before_building() {
    let cfg = XsimConfig {
        vivado_root: PathBuf::from("nowhere"),
        sources: vec![Path::new("nope.v").to_path_buf()],
        top: "top".into(),
        defines: Vec::new(),
        dpi_lib: PathBuf::from("nope.a"),
        work_dir: work("missing"),
        waves: false,
        connect_timeout: Duration::from_secs(1),
        cache_dir: None,
    };
    assert!(matches!(
        XsimPl::start(&cfg),
        Err(backend_rtl_xsim::XsimError::MissingFile(_))
    ));
}

#[test]
fn a_dpi_archive_without_exports_is_rejected_clearly() {
    let Some(vivado) = find_vivado() else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let dir = work("noexports");
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("libxsim_dpi.a");
    std::fs::write(&fake, b"!<arch>\n").unwrap();
    let cfg = XsimConfig {
        vivado_root: vivado,
        sources: vec![fixture("math/my_math_ip.v")],
        top: "cosim_top".into(),
        defines: Vec::new(),
        dpi_lib: fake,
        work_dir: dir.join("w"),
        waves: false,
        connect_timeout: Duration::from_secs(5),
        cache_dir: None,
    };
    match XsimPl::start(&cfg) {
        Err(backend_rtl_xsim::XsimError::Build { step, log }) => {
            assert_eq!(step, "DPI library check");
            assert!(log.contains("cargo xtask dpi"), "{log}");
        }
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("must not start"),
    }
}

/// A design is compiled once: the second start with the same sources reuses
/// the snapshot, and a changed source is compiled again.
#[test]
fn compiled_snapshots_are_cached_by_content() {
    let Some(mut cfg) = config(
        "cache",
        &["math/my_math_ip.v", "math/cosim_top.sv"],
        "cosim_top",
    ) else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let cache = std::env::temp_dir().join(format!("zynqsim-snapshots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    cfg.cache_dir = Some(cache.clone());

    // First start: compiles and stores.
    let t0 = std::time::Instant::now();
    let mut first = XsimPl::start(&cfg).expect("first start");
    let cold = t0.elapsed();
    let log = std::fs::read_to_string(first.work_dir().join("build.log")).unwrap();
    assert!(!log.contains("reused the cached build"), "{log}");
    let probe = |pl: &mut XsimPl| word(pl, 0x43C0_0000, 1000);
    let _ = probe(&mut first);
    first.kill();
    assert_eq!(
        std::fs::read_dir(&cache).unwrap().count(),
        1,
        "one snapshot"
    );

    // Second start, new work directory: reuses it and still works.
    let mut again = cfg.clone();
    again.work_dir = work("cache2");
    let t1 = std::time::Instant::now();
    let mut second = XsimPl::start(&again).expect("second start");
    let warm = t1.elapsed();
    let log = std::fs::read_to_string(second.work_dir().join("build.log")).unwrap();
    assert!(log.contains("reused the cached build"), "{log}");
    let _ = probe(&mut second);
    second.kill();
    eprintln!("cold start {cold:?}, cached start {warm:?}");
    assert!(
        warm < cold,
        "a cached start must be faster: {warm:?} vs {cold:?}"
    );

    // Different defines are a different build.
    let mut other = cfg.clone();
    other.work_dir = work("cache3");
    other.defines = vec!["ZS_CACHE_TEST=1".to_string()];
    let mut third = XsimPl::start(&other).expect("third start");
    let log = std::fs::read_to_string(third.work_dir().join("build.log")).unwrap();
    assert!(!log.contains("reused the cached build"), "{log}");
    third.kill();
    assert_eq!(
        std::fs::read_dir(&cache).unwrap().count(),
        2,
        "two snapshots"
    );
    let _ = std::fs::remove_dir_all(&cache);
}
