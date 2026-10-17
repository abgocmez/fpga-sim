//! Differential check: the same register stimulus on `my_math_ip` in Vivado XSim
//! (through the existing `XsimPl` adapter, as in `backend-rtl-xsim/tests/math.rs`) and
//! in this engine (through `RtlModule`). The register values read back after each
//! write must agree. Needs Vivado; prints `SKIPPED:` and passes without it.

use backend_rtl_xsim::{XsimConfig, XsimPl};
use pl_api::{BusResult, PlTarget, PsMasterPort, PsPort};
use rtl_engine::{Design, RtlModule};
use std::path::PathBuf;
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
        let st = std::process::Command::new(cargo)
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
    let d = std::env::temp_dir().join(format!("zynqsim-rtlengine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
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

const VECTORS: [u32; 8] = [
    0,
    1,
    0x15,
    0x1234_5678,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    0xDEAD_BEEF,
];

#[test]
fn my_math_ip_registers_agree_between_xsim_and_the_engine() {
    let Some(vivado) = find_vivado() else {
        eprintln!("SKIPPED: Vivado/XSim not found (set ZYNQSIM_VIVADO)");
        return;
    };
    let fixture = |rel: &str| workspace().join("fixtures").join("rtl").join(rel);
    let cfg = XsimConfig {
        vivado_root: vivado,
        sources: vec![fixture("math/my_math_ip.v"), fixture("math/cosim_top.sv")],
        top: "cosim_top".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work("xsim-diff"),
        waves: false,
        connect_timeout: Duration::from_secs(180),
        cache_dir: None,
    };
    let mut xsim = XsimPl::start(&cfg).unwrap_or_else(|e| panic!("XSim start failed: {e}"));

    let design = Design::elaborate("my_math_ip", &[fixture("math/my_math_ip.v")], &[], &[]);
    let mut engine = RtlModule::new(design).expect("engine adapter");

    let mut t = 1_000u64;
    let mut xsim_regs = Vec::new();
    let mut engine_regs = Vec::new();
    for v in VECTORS {
        xsim.write(0, 0x43C0_0000, &v.to_le_bytes(), t, &mut NoPs)
            .unwrap();
        t += 100;
        engine.write(0x0, v, 0xF).unwrap();
        for addr in [0x43C0_0000u64, 0x43C0_0004] {
            let mut b = [0u8; 4];
            xsim.read(0, addr, &mut b, t, &mut NoPs).unwrap();
            t += 100;
            xsim_regs.push(u32::from_le_bytes(b));
        }
        engine_regs.push(engine.read(0x0).unwrap());
        engine_regs.push(engine.read(0x4).unwrap());
    }
    // Final values of both registers after the last write, and every read in between.
    assert_eq!(xsim_regs.len(), engine_regs.len());
    for (i, (x, e)) in xsim_regs.iter().zip(&engine_regs).enumerate() {
        assert_eq!(x, e, "register read {i}: XSim {x:#x}, engine {e:#x}");
    }
    let last = VECTORS[VECTORS.len() - 1];
    assert_eq!(xsim_regs[xsim_regs.len() - 1], last.wrapping_mul(2));
    xsim.kill();
}
