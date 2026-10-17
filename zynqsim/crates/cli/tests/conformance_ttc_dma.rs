//! Peripheral conformance of two PS blocks, run on the Xilinx QEMU: the Cadence triple timer
//! counters (xttcps, `examples/conf-ttc`, 14 checks) and the PL330 DMA controller (xdmaps,
//! `examples/conf-dmaps`, 12 checks). Each Rust firmware prints `CHECK n name: STATUS`
//! lines and `RESULT: ALL PASS` only when no check failed. The N/A lists are the checks the
//! QEMU model does not reproduce, see `docs/conformance/ttc-dma.md`.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` and passes without it.

use serde_json::Value;
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

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn qemu_found() -> bool {
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
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

/// One parsed `CHECK n name: STATUS [reason]` line.
#[derive(Debug, PartialEq)]
struct Check {
    n: u32,
    name: String,
    status: String,
}

/// Parses the check lines of a firmware's UART text. A status is the first `: PASS`,
/// `: FAIL` or `: N/A` after the number; names and reasons may contain colons.
fn parse_checks(uart: &str) -> Vec<Check> {
    let mut out = Vec::new();
    for line in uart.lines() {
        let line = line.trim_end_matches('\r');
        let Some(rest) = line.strip_prefix("CHECK ") else {
            continue;
        };
        let Some((num, body)) = rest.split_once(' ') else {
            continue;
        };
        let Ok(n) = num.parse::<u32>() else {
            continue;
        };
        let found = [": PASS", ": FAIL", ": N/A"]
            .iter()
            .filter_map(|m| body.find(m).map(|i| (i, *m)))
            .min_by_key(|(i, _)| *i);
        let Some((i, marker)) = found else {
            continue;
        };
        out.push(Check {
            n,
            name: body[..i].to_string(),
            status: marker[2..].to_string(),
        });
    }
    out
}

/// Runs an example on the QEMU backend and returns its UART-1 text.
fn run_uart(project: &str) -> String {
    let p = root().join(project);
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        p.to_str().unwrap(),
        "--for",
        "200ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["uart"]["1"].as_str().unwrap().to_string()
}

/// Asserts the firmware's self-report: every check numbered 1..=`count` exactly once, no
/// FAIL, `RESULT: ALL PASS`, and the N/A checks are exactly `na` (by number and name).
fn assert_conformance(uart: &str, count: u32, na: &[(u32, &str)]) {
    let checks = parse_checks(uart);
    let numbers: Vec<u32> = checks.iter().map(|c| c.n).collect();
    assert_eq!(
        numbers,
        (1..=count).collect::<Vec<_>>(),
        "check numbers in UART:\n{uart}"
    );
    let fails: Vec<&Check> = checks.iter().filter(|c| c.status == "FAIL").collect();
    assert!(fails.is_empty(), "failed checks: {fails:?}\n{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    let got: Vec<(u32, &str)> = checks
        .iter()
        .filter(|c| c.status == "N/A")
        .map(|c| (c.n, c.name.as_str()))
        .collect();
    assert_eq!(got, na.to_vec(), "N/A checks differ:\n{uart}");
    eprintln!(
        "{} PASS, {} N/A",
        checks.iter().filter(|c| c.status == "PASS").count(),
        got.len()
    );
}

#[test]
fn ttc_conformance_runs_on_the_qemu_model() {
    if !qemu_found() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let uart = run_uart("examples/conf-ttc/project.toml");
    assert_conformance(
        &uart,
        14,
        &[(
            12,
            "Match flag is sticky across reads (cleared only by writing 1)",
        )],
    );
}

#[test]
fn dma_conformance_runs_on_the_qemu_model() {
    if !qemu_found() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let uart = run_uart("examples/conf-dmaps/project.toml");
    assert_conformance(&uart, 12, &[]);
}

#[test]
fn check_lines_parse_with_colons_in_reasons() {
    let text = "CHECK 1 A name: PASS\r\nCHECK 2 B: N/A reason: x\r\nCHECK 3 C: FAIL got 1\r\n";
    let c = parse_checks(text);
    assert_eq!(
        c,
        vec![
            Check {
                n: 1,
                name: "A name".into(),
                status: "PASS".into()
            },
            Check {
                n: 2,
                name: "B".into(),
                status: "N/A".into()
            },
            Check {
                n: 3,
                name: "C".into(),
                status: "FAIL".into()
            },
        ]
    );
}
