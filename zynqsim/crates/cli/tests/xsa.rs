//! `zynqsim init --xsa`: a Vivado hardware handoff becomes a project.
//!
//! The handoffs are real Vivado 2022.1 output (`fixtures/xsa`). The end-to-end test needs the
//! Xilinx QEMU and Vivado XSim and prints `SKIPPED:` without them; the rest needs nothing.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-xsa-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn backend_found(name: &str) -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == name && b["found"] == true)
}

fn ensure_dpi_lib() {
    let lib =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/dpi/release/libxsim_dpi.a");
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
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

/// The design with an AXI GPIO: the project is generated, the report names the unmodelled IP,
/// and `run` stops with the diagnostic instead of simulating a design that is not there.
#[test]
fn an_unmodelled_ip_is_reported_and_stops_the_run() {
    let d = temp("gpio");
    let xsa = fixtures().join("xsa/zs_math_gpio.xsa");
    let rtl = fixtures().join("rtl/math/my_math_ip.v");
    let elf = fixtures().join("elf/math_test.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
        "--rtl",
        rtl.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["design"], "design_1");
    assert_eq!(v["testbench_module"], "my_math_ip");
    assert_eq!(v["runnable"], false);
    let un = v["unmodelled"].as_array().unwrap();
    assert_eq!(un.len(), 1);
    assert_eq!(un[0]["instance"], "axi_gpio_0");
    assert_eq!(un[0]["vlnv"], "xilinx.com:ip:axi_gpio:2.0");
    assert_eq!(un[0]["base"], "0x41200000");
    assert!(
        un[0]["reason"]
            .as_str()
            .unwrap()
            .contains("vendor-ip` model (milestone P3, not built yet)")
    );

    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("\"u_math.S_AXI\" = \"M_AXI_GP0@0x43C00000\""),
        "{toml}"
    );
    assert!(
        toml.contains("\"axi_gpio_0.ip2intc_irpt\" = \"IRQ_F2P[0]\""),
        "{toml}"
    );
    assert!(
        toml.contains("\"u_math.S_AXI_ACLK\" = \"FCLK_CLK0\""),
        "{toml}"
    );
    let report = std::fs::read_to_string(d.join("xsa-report.md")).unwrap();
    assert!(report.contains("NOT RUNNABLE"), "{report}");
    assert!(report.contains("xilinx.com:ip:axi_gpio:2.0"), "{report}");
    assert!(report.contains("0x43C00000"), "{report}");
    assert!(report.contains("IRQ_F2P[0]"), "{report}");
    assert!(d.join("tb/cosim_top.sv").is_file());

    // `run` refuses with the diagnostic and exit code 1 (validation failed).
    let r = zynqsim(&[
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "1ms",
    ]);
    assert_eq!(r.status.code(), Some(1), "{}", stderr(&r));
    let e = stderr(&r);
    assert!(
        e.contains("axi_gpio_0")
            && e.contains("xilinx.com:ip:axi_gpio:2.0")
            && e.contains("0x41200000"),
        "{e}"
    );

    // Refuses to overwrite.
    let again = zynqsim(&[
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
    ]);
    assert_eq!(again.status.code(), Some(2), "{}", stderr(&again));
}

/// Without the RTL the module reference has no source: that is reported, not guessed.
#[test]
fn a_module_reference_without_its_rtl_asks_for_it() {
    let d = temp("nortl");
    let xsa = fixtures().join("xsa/zs_math_only.xsa");
    let o = zynqsim(&[
        "--json",
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], false);
    assert_eq!(v["testbench_module"], Value::Null);
    let un = v["unmodelled"].as_array().unwrap();
    assert_eq!(un.len(), 1);
    assert_eq!(un[0]["instance"], "u_math");
    assert!(un[0]["reason"].as_str().unwrap().contains("--rtl"));
}

#[test]
fn bad_inputs_exit_2_with_a_reason() {
    let d = temp("bad");
    let o = zynqsim(&["init", "--xsa", "no/such.xsa", "--out", d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("cannot read"), "{}", stderr(&o));
    let junk = d.join("junk.xsa");
    std::fs::write(&junk, b"not an archive").unwrap();
    let o = zynqsim(&[
        "init",
        "--xsa",
        junk.to_str().unwrap(),
        "--out",
        d.join("p").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("neither a zip"), "{}", stderr(&o));
    let o = zynqsim(&["init", "--rtl", "x.v", d.join("q").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
}

/// A handoff whose only PL IP is the user's math IP, run on QEMU + XSim with the existing
/// firmware: the answer is right and the bus traffic is the 21 accesses the other routes give.
#[test]
fn a_user_rtl_only_handoff_runs_the_real_firmware() {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("math-only");
    let xsa = fixtures().join("xsa/zs_math_only.xsa");
    let rtl = fixtures().join("rtl/math/my_math_ip.v");
    let elf = fixtures().join("elf/math_test.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
        "--rtl",
        rtl.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true, "{v}");
    assert!(v["unmodelled"].as_array().unwrap().is_empty());

    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    let r: Value = serde_json::from_str(&stdout(&r)).unwrap();
    let uart = r["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(r["bus_accesses"], 21);
}
