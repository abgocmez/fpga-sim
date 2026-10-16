//! AXI protocol checkers (ADR-025) end to end: `init --ip` writes a testbench with the
//! checkers, `zynqsim run --json` reports their findings. Legal designs produce none; the
//! deliberately illegal fixtures in `fixtures/rtl/illegal/` produce the expected rule at the
//! expected time. Needs the Xilinx QEMU and Vivado XSim; without them the tests print
//! `SKIPPED:` and pass.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> PathBuf {
    repo().join("fixtures").join(rel)
}

/// A fresh project directory for `tag`.
fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-protocol-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn backend_found(name: &str) -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    v["backends"]
        .as_array()
        .is_some_and(|bs| bs.iter().any(|b| b["name"] == name && b["found"] == true))
}

/// True when the run can be made; prints the skip reason otherwise.
fn tools_present() -> bool {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return false;
    }
    if !repo().join("target/dpi/release/libxsim_dpi.a").is_file() {
        eprintln!("SKIPPED: the XSim DPI library is missing (cargo xtask dpi)");
        return false;
    }
    true
}

/// `init --ip IP --elf ELF DIR`: the generated testbench and project. Returns the project file.
fn init_ip(ip: &Path, elf: &Path, dir: &Path, module: Option<&str>) -> PathBuf {
    let mut args = vec![
        "init".to_string(),
        "--ip".to_string(),
        ip.display().to_string(),
        "--elf".to_string(),
        elf.display().to_string(),
    ];
    if let Some(m) = module {
        args.push("--module".to_string());
        args.push(m.to_string());
    }
    args.push(dir.display().to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let o = zynqsim(&refs);
    assert!(
        o.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    dir.join("project.toml")
}

/// `run --json` for `for_` of simulated time. Returns the exit code and the JSON report.
fn run_json(project: &Path, for_: &str) -> (i32, Value) {
    let p = project.display().to_string();
    let o = zynqsim(&["run", "--project", &p, "--for", for_, "--json"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "run --json printed no JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    });
    (o.status.code().unwrap_or(-1), v)
}

fn findings(v: &Value) -> Vec<String> {
    v["protocol_checks"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect()
}

/// The firmware that exercises the IP, from `fixtures/elf` (the same one the examples use).
fn elf(name: &str) -> PathBuf {
    fixture(&format!("elf/{name}"))
}

#[test]
fn the_generated_testbench_carries_the_checkers_and_its_checker_file() {
    let d = temp("files");
    let project = init_ip(
        &fixture("rtl/dma/dma_xform.v"),
        &elf("dma_test.elf"),
        &d,
        None,
    );
    let tb = std::fs::read_to_string(d.join("tb/cosim_top.sv")).unwrap();
    assert!(
        tb.contains("zs_chk_lite") && tb.contains("zs_chk_mst"),
        "{tb}"
    );
    assert!(d.join("tb/zs_axi_checker.sv").is_file());
    let toml = std::fs::read_to_string(&project).unwrap();
    assert!(toml.contains("tb/zs_axi_checker.sv"), "{toml}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Legal designs (the math, DMA, IRQ and PMOD OLED fixtures) produce no protocol findings
/// with their generated testbenches. Each run is checked for the `--json` fields.
#[test]
fn legal_designs_produce_zero_protocol_findings() {
    if !tools_present() {
        return;
    }
    let cases = [
        ("rtl/math/my_math_ip.v", None, "math_test.elf", "2ms"),
        ("rtl/dma/dma_xform.v", None, "dma_test.elf", "5ms"),
        ("rtl/irq/irq_ip.v", None, "rust_irq.elf", "4ms"),
        ("rtl/pmod/oled_spi.v", None, "rust_oled.elf", "20ms"),
    ];
    for (ip, module, firmware, for_) in cases {
        let d = temp(&format!("legal-{firmware}"));
        let project = init_ip(&fixture(ip), &elf(firmware), &d, module);
        let (code, v) = run_json(&project, for_);
        let pc = &v["protocol_checks"];
        assert_eq!(code, 0, "{ip}: {v}");
        assert_eq!(pc["mode"], "warn", "{ip}: {v}");
        assert_eq!(pc["warnings"], 0, "{ip}: {v}");
        assert_eq!(pc["errors"], 0, "{ip}: {v}");
        assert!(findings(&v).is_empty(), "{ip}: {v}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Runs an illegal fixture with the math firmware (its writes reach the fixtures' register)
/// and returns the findings.
fn illegal(fixture_name: &str, module: &str, tag: &str) -> Vec<String> {
    let d = temp(tag);
    let project = init_ip(
        &fixture(&format!("rtl/illegal/{fixture_name}")),
        &elf("math_test.elf"),
        &d,
        Some(module),
    );
    let (code, v) = run_json(&project, "2ms");
    assert_eq!(code, 0, "a warning does not fail the run: {v}");
    assert!(
        v["protocol_checks"]["warnings"].as_u64().unwrap() > 0,
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    findings(&v)
}

fn has(list: &[String], text: &str) -> bool {
    list.iter().any(|f| f.contains(text))
}

#[test]
fn a_slave_that_drops_rvalid_before_rready_is_a_valid_drop() {
    if !tools_present() {
        return;
    }
    let f = illegal(
        "lite_rvalid_drop.v",
        "illegal_lite_rvalid_drop",
        "ill-rvalid",
    );
    assert!(
        has(&f, "protocol check warning at ") && has(&f, "AXI4-Lite slave R VALID_DROP"),
        "{f:?}"
    );
    // The finding carries its simulation time, in nanoseconds.
    let first = f.iter().find(|x| x.contains("VALID_DROP")).unwrap();
    let t: u64 = first
        .split(" at ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("a time in ns");
    assert!(t > 0, "{first}");
}

#[test]
fn rvalid_high_during_reset_is_reported_at_the_second_reset_cycle() {
    if !tools_present() {
        return;
    }
    let f = illegal(
        "lite_rvalid_in_reset.v",
        "illegal_lite_rvalid_in_reset",
        "ill-reset",
    );
    assert!(
        has(
            &f,
            "protocol check warning at 15 ns: AXI4-Lite slave R RESET_VALID"
        ),
        "{f:?}"
    );
}

#[test]
fn a_master_that_drops_arvalid_before_arready_is_a_valid_drop() {
    if !tools_present() {
        return;
    }
    let f = illegal(
        "master_arvalid_drop.v",
        "illegal_master_arvalid_drop",
        "ill-arvalid",
    );
    assert!(has(&f, "AXI4 master AR VALID_DROP"), "{f:?}");
}

#[test]
fn a_burst_crossing_4_kb_is_reported_with_its_start_address() {
    if !tools_present() {
        return;
    }
    let f = illegal("master_4kb_cross.v", "illegal_master_4kb_cross", "ill-4kb");
    assert!(has(&f, "AXI4 master AR CROSS_4KB detail=0xff8"), "{f:?}");
}

#[test]
fn a_bad_wlast_is_reported_on_the_beat_where_it_is_wrong() {
    if !tools_present() {
        return;
    }
    let f = illegal(
        "master_bad_wlast.v",
        "illegal_master_bad_wlast",
        "ill-wlast",
    );
    assert!(has(&f, "AXI4 master W LAST_EARLY detail=0x1"), "{f:?}");
    assert!(has(&f, "AXI4 master W LAST_MISSING detail=0x3"), "{f:?}");
}

/// `protocol_checkers = "strict"`: the violations are error records and the run fails.
#[test]
fn strict_checkers_make_a_violation_an_error_and_fail_the_run() {
    if !tools_present() {
        return;
    }
    let d = temp("ill-strict");
    let project = init_ip(
        &fixture("rtl/illegal/master_4kb_cross.v"),
        &elf("math_test.elf"),
        &d,
        Some("illegal_master_4kb_cross"),
    );
    let text = std::fs::read_to_string(&project).unwrap();
    let text = text.replace(
        "top = \"cosim_top\"",
        "top = \"cosim_top\"\nprotocol_checkers = \"strict\"",
    );
    std::fs::write(&project, text).unwrap();
    let (code, v) = run_json(&project, "2ms");
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["protocol_checks"]["mode"], "strict", "{v}");
    assert!(v["protocol_checks"]["errors"].as_u64().unwrap() > 0, "{v}");
    assert!(has(&findings(&v), "protocol check error at "), "{v}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `protocol_checkers = false`: the checkers are compiled out, so the same violation is silent.
#[test]
fn checkers_turned_off_are_not_in_the_build() {
    if !tools_present() {
        return;
    }
    let d = temp("ill-off");
    let project = init_ip(
        &fixture("rtl/illegal/master_4kb_cross.v"),
        &elf("math_test.elf"),
        &d,
        Some("illegal_master_4kb_cross"),
    );
    let text = std::fs::read_to_string(&project).unwrap();
    let text = text.replace(
        "top = \"cosim_top\"",
        "top = \"cosim_top\"\nprotocol_checkers = false",
    );
    std::fs::write(&project, text).unwrap();
    let (code, v) = run_json(&project, "2ms");
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["protocol_checks"]["mode"], "off", "{v}");
    assert!(findings(&v).is_empty(), "{v}");
    let _ = std::fs::remove_dir_all(&d);
}
