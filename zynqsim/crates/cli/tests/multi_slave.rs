//! Multi-slave AXI4-Lite and master bridge end-to-end tests (Task B).
//!
//! Verifies:
//! 1. `zynqsim init --xsa` creates a runnable multi-slave testbench and project from `zs_two_math.xsa`.
//! 2. Two user RTL slaves (`u_math0` at 0x43C00000 and `u_math1` at 0x43C10000) run and answer correctly
//!    under:
//!    - pure Rust in-process `rtl-native` with `ps = "native"`.
//!    - `xsim` with `ps = "qemu"`.
//!    - `verilator` with `ps = "qemu"`.
//!    - `icarus` with `ps = "qemu"`.
//! 3. Protocol checkers run without errors on legal multi-slave designs.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    repo().join("fixtures")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-multi-slave-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn backend_found(name: &str) -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap_or(Value::Null);
    v["backends"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .any(|b| b["name"] == name && b["found"] == true)
}

fn ensure_dpi_lib() {
    let lib = repo().join("target/dpi/release/libxsim_dpi.a");
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
            .current_dir(repo())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

fn init_two_math(dir: &Path) {
    let xsa = fixtures().join("xsa/zs_two_math.xsa");
    let rtl = fixtures().join("rtl/math/my_math_ip.v");
    let elf = fixtures().join("elf/rust_two_math.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
        "--rtl",
        rtl.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "init failed:\n{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true, "{v}");
    assert!(v["unmodelled"].as_array().unwrap().is_empty());
}

#[test]
fn init_xsa_multi_slave_produces_valid_project() {
    let d = temp("init");
    init_two_math(&d);

    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("\"u_math0.S_AXI\" = \"M_AXI_GP0@0x43C00000\""),
        "{toml}"
    );
    assert!(
        toml.contains("\"u_math1.S_AXI\" = \"M_AXI_GP0@0x43C10000\""),
        "{toml}"
    );

    let report = std::fs::read_to_string(d.join("xsa-report.md")).unwrap();
    assert!(report.contains("u_math0"), "{report}");
    assert!(report.contains("u_math1"), "{report}");
    assert!(report.contains("0x43C00000"), "{report}");
    assert!(report.contains("0x43C10000"), "{report}");
    assert!(report.contains("No IP instance is unmodelled"), "{report}");

    let tb = std::fs::read_to_string(d.join("tb/cosim_top.sv")).unwrap();
    assert!(tb.contains("my_math_ip u_math0"), "{tb}");
    assert!(tb.contains("my_math_ip u_math1"), "{tb}");
    assert!(tb.contains("lite_write_0"), "{tb}");
    assert!(tb.contains("lite_write_1"), "{tb}");
    assert!(tb.contains("lite_read_0"), "{tb}");
    assert!(tb.contains("lite_read_1"), "{tb}");
    assert!(tb.contains("zs_chk_lite_0"), "{tb}");
    assert!(tb.contains("zs_chk_lite_1"), "{tb}");
    assert!(tb.contains("32'h43C00000"), "{tb}");
    assert!(tb.contains("32'h43C10000"), "{tb}");
}

#[test]
fn multi_slave_runs_on_rtl_native() {
    let d = temp("rtl-native");
    init_two_math(&d);

    let ppath = d.join("project.toml");
    let toml = std::fs::read_to_string(&ppath).unwrap();
    let updated = toml
        .replace("ps = \"qemu\"", "ps = \"native\"")
        .replace("pl = \"xsim\"", "pl = \"rtl-native\"");
    std::fs::write(&ppath, updated).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        ppath.to_str().unwrap(),
        "--for",
        "20ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "native");
    assert_eq!(v["backend"]["pl"], "rtl-native");
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("math0: 21 * 2 = 42"), "{uart}");
    assert!(uart.contains("math1: 50 * 2 = 100"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert!(uart.contains("COSIM_DONE"), "{uart}");
    assert_eq!(v["protocol_checks"]["errors"], 0);
    assert_eq!(v["bus_accesses"], 5);
}

#[test]
fn multi_slave_runs_on_xsim() {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("xsim");
    init_two_math(&d);

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "qemu");
    assert_eq!(v["backend"]["pl"], "xsim");
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("math0: 21 * 2 = 42"), "{uart}");
    assert!(uart.contains("math1: 50 * 2 = 100"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(v["protocol_checks"]["errors"], 0);
    assert_eq!(v["bus_accesses"], 5);
}

#[test]
fn multi_slave_runs_on_verilator() {
    if !backend_found("qemu") || !backend_found("verilator") {
        eprintln!("SKIPPED: needs Xilinx QEMU and Verilator");
        return;
    }
    ensure_dpi_lib();
    let d = temp("verilator");
    init_two_math(&d);

    let ppath = d.join("project.toml");
    let toml = std::fs::read_to_string(&ppath).unwrap();
    std::fs::write(&ppath, toml.replace("pl = \"xsim\"", "pl = \"verilator\"")).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        ppath.to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "qemu");
    assert_eq!(v["backend"]["pl"], "verilator");
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("math0: 21 * 2 = 42"), "{uart}");
    assert!(uart.contains("math1: 50 * 2 = 100"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(v["protocol_checks"]["errors"], 0);
    assert_eq!(v["bus_accesses"], 5);
}

#[test]
fn multi_slave_runs_on_icarus() {
    if !backend_found("qemu") || !backend_found("icarus") {
        eprintln!("SKIPPED: needs Xilinx QEMU and Icarus Verilog");
        return;
    }
    let d = temp("icarus");
    init_two_math(&d);

    let ppath = d.join("project.toml");
    let toml = std::fs::read_to_string(&ppath).unwrap();
    std::fs::write(&ppath, toml.replace("pl = \"xsim\"", "pl = \"icarus\"")).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        ppath.to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "qemu");
    assert_eq!(v["backend"]["pl"], "icarus");
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("math0: 21 * 2 = 42"), "{uart}");
    assert!(uart.contains("math1: 50 * 2 = 100"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(v["protocol_checks"]["errors"], 0);
    assert_eq!(v["bus_accesses"], 5);
}
