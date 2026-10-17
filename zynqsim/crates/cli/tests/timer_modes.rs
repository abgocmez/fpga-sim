//! AXI Timer capture and cascade modes (ADR-026) on the real firmware: `examples/timer-modes`
//! runs `firmware/timer_modes` on the Xilinx QEMU with the vendor-ip timer model. The board
//! button 0 is bound to `CAPTURETRIG0` and the test presses it with `--at`.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` without it.

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

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/timer-modes/project.toml")
}

/// One run of the example: the button is pressed at 3 ms and released at 4 ms.
fn run_once() -> Value {
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        project().to_str().unwrap(),
        "--for",
        "20ms",
        "--at",
        "3ms=btn0:down",
        "--at",
        "4ms=btn0:up",
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    serde_json::from_str(&stdout(&o)).unwrap()
}

/// The value after `key=` in a firmware line, as a number (`tlr=0x48ae4`).
fn hex_field(line: &str, key: &str) -> u64 {
    let rest = line
        .split(&format!("{key}="))
        .nth(1)
        .unwrap_or_else(|| panic!("no `{key}=` in {line}"));
    let text = rest.split_whitespace().next().unwrap();
    u64::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

#[test]
fn capture_and_cascade_modes_run_on_the_real_firmware() {
    if !backend_found("qemu") {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let v = run_once();
    let u1 = v["uart"]["1"].as_str().unwrap();
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");

    // Capture: a rising edge on board button 0 at 3 ms copied the counter into TLR0 (about
    // 2.98 ms of clocks after the timer was armed at start-up), once, and the release did not
    // capture again.
    let capture = u1
        .lines()
        .find(|l| l.starts_with("capture "))
        .unwrap_or_else(|| panic!("{u1}"));
    assert!(capture.contains("events=1"), "{capture}");
    assert!(capture.contains("capture_ok=1"), "{capture}");
    let tlr = hex_field(capture, "tlr");
    let tcr = hex_field(capture, "tcr");
    assert!(
        (250_000..=300_000).contains(&tlr),
        "3 ms of clocks at 100 MHz, less the start-up: {tlr}"
    );
    assert!(tcr > tlr, "the counter ran on after the capture: {capture}");

    // Cascade: the low word wrapped past 2^32 and the high word became 1; no interrupt.
    assert!(u1.contains("cascade carry tcr1=0x1"), "{u1}");
    assert!(u1.contains("carry_ok=1"), "{u1}");
    // Cascade periodic: three 64-bit rollover interrupts and the high word stayed all ones.
    assert!(
        u1.contains("cascade periodic irqs=3 tcr1=0xffffffff periodic_ok=1"),
        "{u1}"
    );

    // The same inputs give the same run: trace hash and end time (ADR-011 determinism).
    let again = run_once();
    assert_eq!(v["trace_hash"], again["trace_hash"], "{v} vs {again}");
    assert_eq!(v["now_ps"], again["now_ps"]);
    assert_eq!(v["uart"]["1"], again["uart"]["1"]);
}

#[test]
fn a_capture_input_that_is_a_whole_bus_is_refused_before_the_run() {
    // Needs no QEMU: the project is validated before any engine starts.
    let dir = std::env::temp_dir().join(format!("zynqsim-timer-modes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("project.toml");
    // The copy lives in the temp directory, so the firmware path must be absolute.
    // Forward slashes: a backslash starts an escape in a TOML basic string.
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .to_string_lossy()
        .replace('\\', "/");
    let text = std::fs::read_to_string(project())
        .unwrap()
        .replace(
            "\"axi_timer_0.CAPTURETRIG0\" = \"board.btn0\"",
            "\"axi_timer_0.CAPTURETRIG0\" = \"board.btn\"",
        )
        .replace("../../fixtures", &fixtures);
    std::fs::write(&file, text).unwrap();
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        file.to_str().unwrap(),
        "--for",
        "1ms",
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "{}", stdout(&o));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(all.contains("CAPTURETRIG0"), "{all}");
    assert!(all.contains("board.swN or board.btnN"), "{all}");
}
