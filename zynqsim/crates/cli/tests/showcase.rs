//! The showcase (`docs/SHOWCASE.md`): a Vivado block design (the user's 8-tap FIR in Verilog plus
//! an AXI GPIO pair, Timer, UART Lite and Quad SPI) and Rust firmware, end to end.
//!
//! `fixtures/showcase` holds the design as an XSA and as an `export_simulation` folder and the
//! `BOOT.BIN` `bootgen` packed. The `init` and the native-route tests need no external tool; the
//! QEMU + XSim test needs the Xilinx QEMU and Vivado XSim and prints `SKIPPED:` without them.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-showcase-{tag}-{}", std::process::id()));
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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> PathBuf {
    root().join("fixtures/showcase").join(rel)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The UART1 lines the firmware prints, in this order (`firmware/showcase_demo`).
const LINES: [&str; 11] = [
    "ZYNQSIM SHOWCASE",
    "fir id ok",
    "fir lowpass: 64 samples hw==sw",
    "fir irq count: 64",
    "fir highpass: 64 samples hw==sw",
    "leds walked",
    "switches: 0x0",
    "buttons: 0x0",
    "timer irq count: 3",
    "spi loopback: 16 bytes ok",
    "uartlite sent",
];

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
    let lib = root().join("target/dpi/release/libxsim_dpi.a");
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
            .current_dir(root())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

fn init(dir: &Path, extra: &[&str]) -> Value {
    let xsa = fixture("design.xsa");
    let rtl = fixture("rtl/zs_fir.v");
    let mut args = vec![
        "--json",
        "init",
        "--xsa",
        s(&xsa),
        "--rtl",
        s(&rtl),
        "--out",
        s(dir),
    ];
    args.extend(extra);
    let o = zynqsim(&args);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    serde_json::from_str(&stdout(&o)).unwrap()
}

fn run(project: &Path, duration: &str) -> Value {
    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        s(&project.join("project.toml")),
        "--for",
        duration,
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    serde_json::from_str(&stdout(&r)).unwrap()
}

/// What the firmware must have printed and what the PL must have shown.
fn assert_all_pass(r: &Value) {
    let u1 = r["uart"]["1"].as_str().unwrap();
    let mut from = 0;
    for line in LINES {
        match u1[from..].find(line) {
            Some(i) => from += i + line.len(),
            None => panic!("UART1 lacks `{line}` (in order):\n{u1}"),
        }
    }
    assert!(!u1.contains("FAIL"), "{u1}");
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");
    // The UART Lite line, as the PL sent it (port 2 = the first UART Lite).
    assert_eq!(r["uart"]["pl"]["2"], "SHOWCASE PL UART OK\r\n");
    // GPIO 0x9 on LED0 and LED3.
    let duty: Vec<u64> = r["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["duty"].as_u64().unwrap())
        .collect();
    assert_eq!(duty, vec![255, 0, 0, 255], "{r}");
}

/// One `init` maps every block of the design; nothing is left without a model. Needs no tools.
#[test]
fn the_showcase_design_is_fully_mapped() {
    let d = temp("init");
    let bin = fixture("BOOT.BIN");
    let v = init(&d, &["--boot", s(&bin)]);
    assert_eq!(v["runnable"], true, "{v}");
    assert_eq!(v["boot"]["mode"], "boot_bin");
    assert_eq!(v["testbench_module"], "zs_fir");
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    for needle in [
        "\"zs_fir.S_AXI\" = \"M_AXI_GP0@0x43C00000\"",
        "\"axi_gpio_0.S_AXI\" = \"M_AXI_GP0@0x41200000\"",
        "\"axi_gpio_1.S_AXI\" = \"M_AXI_GP0@0x41210000\"",
        "\"axi_timer_0.S_AXI\" = \"M_AXI_GP0@0x42800000\"",
        "\"axi_uartlite_0.S_AXI\" = \"M_AXI_GP0@0x40600000\"",
        "\"axi_quad_spi_0.AXI_LITE\" = \"M_AXI_GP0@0x41E00000\"",
        "\"zs_fir.irq\" = \"IRQ_F2P[0]\"",
        "\"axi_gpio_0.ip2intc_irpt\" = \"IRQ_F2P[1]\"",
        "\"axi_gpio_1.ip2intc_irpt\" = \"IRQ_F2P[2]\"",
        "\"axi_timer_0.interrupt\" = \"IRQ_F2P[3]\"",
        "\"axi_uartlite_0.interrupt\" = \"IRQ_F2P[4]\"",
        "\"axi_quad_spi_0.ip2intc_irpt\" = \"IRQ_F2P[5]\"",
        "\"axi_gpio_0.GPIO\" = \"board.leds\"",
        "\"axi_gpio_0.GPIO2\" = \"board.sw\"",
        "\"axi_gpio_1.GPIO\" = \"board.btn\"",
        "axi_quad_spi_0 = \"axi_quad_spi\"",
    ] {
        assert!(toml.contains(needle), "missing `{needle}`:\n{toml}");
    }
    assert!(toml.contains("unmodelled = []"), "{toml}");
}

/// The whole design on the native route: native Cortex-A9, the FIR RTL in the pure-Rust RTL
/// engine and the Xilinx IP as models. No Xilinx tool, no external simulator.
#[test]
fn the_showcase_runs_natively_without_any_tool() {
    let d = temp("native");
    let v = init(&d, &["--ps", "native", "--pl", "rtl-native"]);
    // The native PS boots an ELF (not a BOOT.BIN): point the project at the firmware.
    let _ = v;
    let toml_path = d.join("project.toml");
    let toml = std::fs::read_to_string(&toml_path).unwrap();
    let elf = root().join("fixtures/elf/rust_showcase.elf");
    let toml = toml.replace(
        "elf = \"fw/app.elf\"",
        &format!("elf = \"{}\"", s(&elf).replace('\\', "/")),
    );
    std::fs::write(&toml_path, toml).unwrap();
    let r = run(&d, "40ms");
    assert_all_pass(&r);
    assert_eq!(r["backend"]["ps"], "native");
    // The same project twice: the same trace digest.
    let r2 = run(&d, "40ms");
    assert_eq!(r["trace_hash"], r2["trace_hash"]);
}

/// Real Xilinx QEMU boots the BOOT.BIN; the FIR RTL runs in Vivado XSim with the protocol
/// checkers; the Xilinx IP are the models. Three runs, one trace digest.
#[test]
fn the_showcase_runs_from_the_bootbin_on_qemu_and_xsim() {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("xsim");
    let bin = fixture("BOOT.BIN");
    init(&d, &["--boot", s(&bin)]);
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let r = run(&d, "40ms");
        assert_all_pass(&r);
        assert!(
            r["uart"]["1"].as_str().unwrap().contains("COSIM_DONE"),
            "{r}"
        );
        assert_eq!(r["backend"]["pl"], "xsim+native");
        hashes.push(r["trace_hash"].clone());
    }
    assert!(hashes[0].is_string(), "{hashes:?}");
    assert_eq!(hashes[0], hashes[1], "one trace digest");
}
