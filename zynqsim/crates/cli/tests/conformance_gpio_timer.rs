//! Conformance of the PS GPIO (xgpiops) and the Cortex-A9 timers (xscutimer
//! and the global timer), run through the `zynqsim` binary on the Xilinx QEMU.
//! The Rust firmware prints one `CHECK n name: PASS|FAIL|N/A why` line per
//! register-level expectation and `RESULT: ALL PASS` when no check failed.
//!
//! The test pins the N/A list: a check that QEMU does not model (or models
//! differently from the TRM) is N/A with a reason in the firmware, and a new
//! model or a regression changes this list, which fails the test on purpose.
//! The table with the observed values is docs/conformance/gpio-timer.md.
//!
//! They need the Xilinx QEMU; without it they print `SKIPPED:` and return.

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

fn example(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join("project.toml")
        .to_string_lossy()
        .into_owned()
}

/// Runs an example and returns its UART1 text.
fn uart_of(name: &str, duration: &str) -> String {
    let project = example(name);
    let o = zynqsim(&["--json", "run", "--project", &project, "--for", duration]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "qemu");
    v["uart"]["1"].as_str().unwrap().to_string()
}

/// Checks the firmware's report: every check numbered in order, no FAIL, the
/// N/A checks are exactly `expected_na` (by name), and the final result is
/// `ALL PASS`. Returns the number of PASS lines.
fn assert_conformance(uart: &str, checks: usize, expected_na: &[&str]) -> usize {
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    let lines: Vec<&str> = uart
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| l.starts_with("CHECK "))
        .collect();
    assert_eq!(lines.len(), checks, "check count\n{uart}");

    let mut na = Vec::new();
    let mut pass = 0;
    for (i, line) in lines.iter().enumerate() {
        let (num, rest) = line["CHECK ".len()..].split_once(' ').unwrap();
        assert_eq!(num, (i + 1).to_string(), "check numbering\n{uart}");
        let (name, status) = rest.split_once(": ").unwrap_or_else(|| panic!("{line}"));
        if status == "PASS" {
            pass += 1;
        } else if status.starts_with("N/A ") {
            assert!(status.len() > "N/A ".len(), "N/A needs a reason: {line}");
            na.push(name.to_string());
        } else {
            panic!("check did not pass: {line}\n{uart}");
        }
    }
    assert_eq!(na, expected_na, "N/A list\n{uart}");
    pass
}

/// xgpiops: QEMU maps no GPIO at 0xE000A000 (every register check is N/A).
const GPIO_CHECKS: usize = 12;
const GPIO_NA: &[&str] = &[
    "reset_values",
    "dirm_readback",
    "oen_readback",
    "data_readback",
    "masked_write_lsw",
    "masked_write_msw",
    "bank2_width",
    "int_mask_en_dis",
    "int_type_polarity_any",
    "int_status_no_events",
    "data_ro_mirrors_output",
    "pin_event_interrupt",
];

/// Private timer, timer interrupts through the GIC, and the global timer. Only
/// the three global-timer checks are N/A (QEMU deviations, partly modelled).
const TIMER_CHECKS: usize = 17;
const TIMER_NA: &[&str] = &[
    "gt_counter_writable_when_stopped",
    "gt_counter_holds_when_disabled",
    "gt_comparator_flag",
];

#[test]
fn conf_gpiops_checks_match_the_expected_na_list() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let uart = uart_of("conf-gpiops", "20ms");
    let pass = assert_conformance(&uart, GPIO_CHECKS, GPIO_NA);
    assert_eq!(
        pass, 0,
        "no GPIO check can pass while QEMU maps no GPIO\n{uart}"
    );
}

#[test]
fn conf_scutimer_checks_match_the_expected_na_list() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let uart = uart_of("conf-scutimer", "200ms");
    let pass = assert_conformance(&uart, TIMER_CHECKS, TIMER_NA);
    assert_eq!(pass, TIMER_CHECKS - TIMER_NA.len(), "{uart}");
}
