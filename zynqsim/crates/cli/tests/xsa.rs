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
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
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

/// The design with an AXI HWICAP (no model): the project is generated, the report names the
/// unmodelled IP, and `run` stops with the diagnostic instead of simulating a design that is
/// not there.
#[test]
fn an_unmodelled_ip_is_reported_and_stops_the_run() {
    let d = temp("hwicap");
    let xsa = fixtures().join("xsa/zs_math_hwicap.hwh");
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
    assert_eq!(v["design"], "design_hwicap");
    assert_eq!(v["testbench_module"], "my_math_ip");
    assert_eq!(v["runnable"], false);
    let un = v["unmodelled"].as_array().unwrap();
    assert_eq!(un.len(), 1);
    assert_eq!(un[0]["instance"], "axi_hwicap_0");
    assert_eq!(un[0]["vlnv"], "xilinx.com:ip:axi_hwicap:3.0");
    assert_eq!(un[0]["base"], "0x43000000");
    assert!(
        un[0]["reason"]
            .as_str()
            .unwrap()
            .contains("`vendor-ip` model that does not exist yet")
    );

    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("\"u_math.S_AXI\" = \"M_AXI_GP0@0x43C00000\""),
        "{toml}"
    );
    assert!(
        toml.contains("\"axi_hwicap_0.ip2intc_irpt\" = \"IRQ_F2P[0]\""),
        "{toml}"
    );
    assert!(
        toml.contains("\"u_math.S_AXI_ACLK\" = \"FCLK_CLK0\""),
        "{toml}"
    );
    let report = std::fs::read_to_string(d.join("xsa-report.md")).unwrap();
    assert!(report.contains("NOT RUNNABLE"), "{report}");
    assert!(report.contains("xilinx.com:ip:axi_hwicap:3.0"), "{report}");
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
        e.contains("axi_hwicap_0")
            && e.contains("xilinx.com:ip:axi_hwicap:3.0")
            && e.contains("0x43000000"),
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

#[test]
fn the_vdma_design_maps_the_vdma_model() {
    let d = temp("vdma_map");
    let xsa = fixtures().join("xsa/zs_math_vdma.hwh");
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
    assert_eq!(v["design"], "design_vdma");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("axi_vdma_0 = \"axi_vdma\""), "{toml}");
}

#[test]
fn the_ethlite_design_maps_the_ethlite_model() {
    let d = temp("ethlite_map");
    let xsa = fixtures().join("xsa/zs_math_ethlite.hwh");
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
    assert_eq!(v["design"], "design_ethlite");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("axi_ethernetlite_0 = \"axi_ethernetlite\""),
        "{toml}"
    );
}

#[test]
fn the_vtc_design_maps_the_vtc_model() {
    let d = temp("vtc_map");
    let xsa = fixtures().join("xsa/zs_math_vtc.hwh");
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
    assert_eq!(v["design"], "design_vtc");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("v_tc_0 = \"v_tc\""), "{toml}");
}

#[test]
fn the_cdma_design_maps_the_cdma_model() {
    let d = temp("cdma_map");
    let xsa = fixtures().join("xsa/zs_math_cdma.hwh");
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
    assert_eq!(v["design"], "design_cdma");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("axi_cdma_0 = \"axi_cdma\""), "{toml}");
}

#[test]
fn the_uart16550_design_maps_the_uart16550_model() {
    let d = temp("uart16550_map");
    let xsa = fixtures().join("xsa/zs_math_uart16550.hwh");
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
    assert_eq!(v["design"], "design_uart16550");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("axi_uart16550_0 = \"axi_uart16550\""),
        "{toml}"
    );
}

#[test]
fn the_vid_out_design_maps_the_vid_out_model() {
    let d = temp("vid_out_map");
    let xsa = fixtures().join("xsa/zs_math_vid_out.hwh");
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
    assert_eq!(v["design"], "design_vid_out");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("v_axi4s_vid_out_0 = \"v_axi4s_vid_out\""),
        "{toml}"
    );
}

#[test]
fn the_vid_in_design_maps_the_vid_in_model() {
    let d = temp("vid_in_map");
    let xsa = fixtures().join("xsa/zs_math_vid_in.hwh");
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
    assert_eq!(v["design"], "design_vid_in");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("v_vid_in_axi4s_0 = \"v_vid_in_axi4s\""),
        "{toml}"
    );
}

#[test]
fn the_ila_design_maps_the_ila_model() {
    let d = temp("ila_map");
    let xsa = fixtures().join("xsa/zs_math_ila.hwh");
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
    assert_eq!(v["design"], "design_ila");
    assert_eq!(v["runnable"], true);
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("ila_0 = \"ila\""), "{toml}");
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

/// A handoff made only of Xilinx IP (real Vivado 2022.1 output: two AXI GPIOs, an AXI Timer and an
/// AXI UART Lite) imports without unmodelled IP and runs the real firmware on QEMU alone: the
/// vendor-ip models answer, the timer interrupt is taken, the UART Lite's bytes reach the host.
#[test]
fn a_vendor_ip_handoff_runs_the_real_firmware_on_the_models() {
    if !backend_found("qemu") {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let d = temp("vendor");
    let xsa = fixtures().join("xsa/zs_vendor_ip.xsa");
    let elf = fixtures().join("elf/rust_gpio_timer_uart.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        "--xsa",
        xsa.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true, "{v}");
    assert!(v["unmodelled"].as_array().unwrap().is_empty());
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("axi_timer_0 = \"axi_timer\""), "{toml}");
    assert!(
        toml.contains("\"axi_gpio_0.GPIO\" = \"board.leds\""),
        "{toml}"
    );
    assert!(toml.contains("pl = \"native\""), "{toml}");

    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "6ms",
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    let r: Value = serde_json::from_str(&stdout(&r)).unwrap();
    let u1 = r["uart"]["1"].as_str().unwrap();
    assert!(u1.contains("timer ticks=3"), "{u1}");
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");
    assert_eq!(
        r["uart"]["pl"]["2"].as_str().unwrap(),
        "Hello from AXI UART Lite\r\n"
    );
    assert_eq!(r["backend"]["pl"], "native");
}

/// The user's RTL (XSim) and an AXI GPIO model share one PL: accesses in the GPIO's window go to
/// the model, the rest to the RTL, and the model's output reaches the board LEDs.
#[test]
fn user_rtl_in_xsim_and_a_vendor_model_share_one_pl() {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("mixed");
    let xsa = fixtures().join("xsa/zs_math_gpio.xsa");
    let rtl = fixtures().join("rtl/math/my_math_ip.v");
    let elf = fixtures().join("elf/rust_vendor_mix.elf");
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

    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "5ms",
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    let r: Value = serde_json::from_str(&stdout(&r)).unwrap();
    let u1 = r["uart"]["1"].as_str().unwrap();
    assert!(u1.contains("math: 0x15 * 2 = 0x2a"), "{u1}");
    assert!(u1.contains("gpio: tri=0x0 data=0x9"), "{u1}");
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");
    assert_eq!(r["backend"]["pl"], "xsim+native");
    let duty: Vec<u64> = r["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["duty"].as_u64().unwrap())
        .collect();
    assert_eq!(
        duty,
        vec![255, 0, 0, 255],
        "LED0 and LED3 from GPIO bits 0 and 3"
    );
}
