//! Conformance of the PS I2C (xiicps), SPI (xspips) and XADC (xadcps) blocks: the
//! Rust firmware in `firmware/conf_iicps`, `conf_spips` and `conf_xadcps` runs on the
//! Xilinx QEMU and prints one `CHECK n name: PASS|FAIL|N/A reason` line per check.
//!
//! The test asserts that each run ends with `RESULT: ALL PASS`, that no check FAILs,
//! and that the N/A checks are exactly the listed ones (each N/A is a documented
//! deviation of QEMU's model or a check that is not made, with its reason on the
//! UART; see `docs/conformance/iic-spi-xadc.md`).
//!
//! Tests that need QEMU print `SKIPPED:` and return when it is not installed.

use serde_json::Value;
use std::path::PathBuf;
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

fn qemu_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs an example project for `duration` and returns its UART1 text.
fn uart1_of(example: &str, duration: &str) -> String {
    let project = root().join("examples").join(example).join("project.toml");
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &project.to_string_lossy(),
        "--for",
        duration,
    ]);
    assert_eq!(o.status.code(), Some(0), "{example}: {}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["uart"]["1"].as_str().unwrap().replace("\r\n", "\n")
}

/// The `CHECK` lines of a UART text, as (number, name, verdict) where the verdict
/// starts with `PASS`, `FAIL` or `N/A` and carries the reason. Names may contain
/// `: ` themselves, so the verdict is the first `: PASS`, `: FAIL` or `: N/A `.
fn checks(uart: &str) -> Vec<(u32, String, String)> {
    uart.lines()
        .filter_map(|l| l.strip_prefix("CHECK "))
        .map(|rest| {
            let (num, rest) = rest.split_once(' ').expect("CHECK number");
            let at = [": PASS", ": FAIL", ": N/A "]
                .iter()
                .filter_map(|m| rest.find(m))
                .min()
                .expect("CHECK verdict");
            (
                num.parse().expect("number"),
                rest[..at].to_string(),
                rest[at + 2..].to_string(),
            )
        })
        .collect()
}

/// Asserts the run passes, has no FAIL, and its N/A checks are exactly `expected`
/// (number and name). Returns the number of checks.
fn assert_conformance(example: &str, duration: &str, expected_na: &[(u32, &str)]) -> usize {
    let uart = uart1_of(example, duration);
    assert!(
        uart.contains("RESULT: ALL PASS"),
        "{example} did not pass:\n{uart}"
    );
    assert!(!uart.contains("RESULT: FAIL"), "{example}:\n{uart}");
    let all = checks(&uart);
    assert!(!all.is_empty(), "{example} printed no checks:\n{uart}");
    for (n, name, verdict) in &all {
        assert!(
            !verdict.starts_with("FAIL"),
            "{example}: check {n} {name} failed: {verdict}"
        );
    }
    let na: Vec<(u32, &str)> = all
        .iter()
        .filter(|(_, _, v)| v.starts_with("N/A "))
        .map(|(n, name, _)| (*n, name.as_str()))
        .collect();
    assert_eq!(
        na,
        expected_na.to_vec(),
        "{example}: N/A checks differ:\n{uart}"
    );
    all.len()
}

#[test]
fn iic_spi_xadc_firmware_passes_with_the_listed_na_checks() {
    if !qemu_available() {
        println!("SKIPPED: the Xilinx QEMU (Vitis) is not installed");
        return;
    }

    // I2C0 and I2C1: the attached-device read is N/A (none attached in this
    // example), and the two TIMEOUT reset values differ from the remembered UG585
    // value (see the conformance table).
    let i2c = assert_conformance(
        "conf-iicps",
        "50ms",
        &[
            (5, "I2C0 TIMEOUT reset"),
            (18, "I2C0 read from attached device"),
            (20, "I2C1 TIMEOUT reset"),
        ],
    );
    assert_eq!(i2c, 22);

    // SPI0 and SPI1: the probe finds the register window reads 0 and ignores writes,
    // so every register check is N/A (not modelled), and the GIC line is not checked.
    let spi_na: Vec<(u32, &str)> = vec![
        (1, "SPI0 Config reset"),
        (2, "SPI0 Enable reset (disabled)"),
        (3, "SPI0 Delay reset"),
        (4, "SPI0 TX threshold reset"),
        (5, "SPI0 RX threshold reset"),
        (6, "SPI0 Slave_Idle_count reset"),
        (7, "SPI0 empty TX FIFO: TX_FIFO_not_full set"),
        (8, "SPI0 empty TX FIFO: TX_FIFO_full clear"),
        (9, "SPI0 interrupt mask reset (all masked)"),
        (10, "SPI0 Config write/read (master, CPHA, baud)"),
        (11, "SPI0 manual start enable, SS force and CS0 read back"),
        (12, "SPI0 Delay write/read"),
        (13, "SPI0 TX threshold write/read"),
        (14, "SPI0 RX threshold write/read"),
        (15, "SPI0 Slave_Idle_count write/read"),
        (16, "SPI0 IER/IDR move the TX_FIFO_not_full mask"),
        (17, "SPI0 Enable write/read (1)"),
        (18, "SPI0 Enable write/read (0)"),
        (19, "SPI0 TX FIFO full after 128 writes: TX_FIFO_full set"),
        (20, "SPI0 full TX FIFO: TX_FIFO_not_full clear"),
        (
            21,
            "SPI0 manual start drains the TX FIFO (TX_FIFO_full clear)",
        ),
        (22, "SPI0 RX FIFO not empty after the transfer"),
        (23, "SPI0 loopback of the first byte with no slave attached"),
        (24, "SPI0 Delay unchanged by a write to SPI1"),
        (25, "SPI1 Delay write/read"),
        (26, "SPI1 Config reset"),
        (27, "SPI0 interrupt line to the GIC"),
    ];
    let spi = assert_conformance("conf-spips", "50ms", &spi_na);
    assert_eq!(spi, 27);

    // XADC: four reset values differ from the remembered UG585 values, the conversion
    // reads return 0 (no data), and two checks are not made.
    let xadc = assert_conformance(
        "conf-xadcps",
        "50ms",
        &[
            (1, "XADC CFG reset"),
            (2, "XADC INT_STS reset"),
            (3, "XADC MSTS reset"),
            (4, "XADC MCTL reset"),
            (9, "XADC INT_STS write-1-to-clear"),
            (10, "XADC temperature via CMDFIFO/RDFIFO in -40..125 C"),
            (11, "XADC VCCINT via CMDFIFO/RDFIFO in 0.9..1.1 V"),
            (12, "XADC DRP write then read (0x40) returns the value"),
            (13, "XADC alarm thresholds (DRP 0x40..0x4F) beyond 0x40"),
        ],
    );
    assert_eq!(xadc, 13);
}
