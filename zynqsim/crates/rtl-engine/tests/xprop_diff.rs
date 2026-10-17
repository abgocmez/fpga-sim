//! X-propagation differential: `tests/xprop/xprop_tb.v` run in Vivado XSim (standalone) and
//! in the native engine. Every check prints `R <name> <value>` with `%b`; the two outputs must
//! be identical line by line. Skipped with `SKIPPED:` when Vivado is not installed.

use backend_rtl_xsim::{StandaloneConfig, StandaloneSource, run_standalone};
use rtl_engine::{Design, Simulator};
use std::path::PathBuf;

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

fn tb_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("xprop")
        .join("xprop_tb.v")
}

fn r_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| l.starts_with("R "))
        .collect()
}

#[test]
fn x_propagation_testbench_agrees_between_xsim_and_the_engine() {
    let Some(vivado) = find_vivado() else {
        eprintln!("SKIPPED: Vivado is not installed");
        return;
    };
    let tb = tb_path();

    // The native engine.
    let design = Design::elaborate("xprop_tb", std::slice::from_ref(&tb), &[], &[]);
    let mut sim: Simulator = Simulator::new(design).unwrap_or_else(|d| {
        panic!(
            "engine refused the testbench: {:?}",
            d.iter().map(|x| x.to_string()).collect::<Vec<_>>()
        )
    });
    sim.run_until(1_000_000).expect("engine run");
    let engine = sim.log().to_vec();

    // XSim, standalone.
    let work = std::env::temp_dir().join("zynqsim-xprop-diff");
    let mut cfg = StandaloneConfig::new(vivado, "xprop_tb", work);
    cfg.sources = vec![StandaloneSource::auto(&tb)];
    let report = run_standalone(&cfg).expect("standalone run");
    assert!(
        report.compiled && report.elaborated && report.ran,
        "XSim: {:?}",
        report.failed_stage()
    );
    let xsim = r_lines(&report.sim_output);

    let engine_r: Vec<String> = engine.into_iter().filter(|l| l.starts_with("R ")).collect();
    assert!(
        engine_r.len() >= 30,
        "the testbench printed {} checks",
        engine_r.len()
    );
    assert_eq!(engine_r, xsim, "engine and XSim differ");
}
