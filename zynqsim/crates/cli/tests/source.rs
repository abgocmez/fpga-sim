//! `zynqsim run --break file.rs:LINE`: the run stops at a source line and the
//! stop is reported with its file and line.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn mark(name: &str) -> u64 {
    std::fs::read_to_string(root().join("firmware/debug_demo/src/main.rs"))
        .unwrap()
        .lines()
        .position(|l| l.contains(&format!("MARK:{name}")))
        .map(|i| i as u64 + 1)
        .unwrap_or_else(|| panic!("MARK:{name}"))
}

#[test]
fn break_at_a_source_line_reports_file_and_line() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let project = root().join("examples/rust-debug/project.toml");
    let project = project.to_string_lossy().into_owned();
    let line = mark("call-collatz");
    // The project's own `main.rs:LINE` breakpoint (inside the loop) is also
    // set; the call line comes first in execution order.
    let loc = format!("main.rs:{line}");
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &project,
        "--for",
        "1ms",
        "--break",
        &loc,
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let d = &v["debug"];
    assert_eq!(d["stopped"], true, "{v}");
    assert_eq!(d["source"]["line"], line, "{d}");
    assert!(
        d["source"]["file"]
            .as_str()
            .unwrap()
            .ends_with("debug_demo/src/main.rs")
    );
    assert!(
        d["reason"]
            .as_str()
            .unwrap()
            .contains(&format!("main.rs:{line}"))
    );

    // Text output names the line as well.
    let o = zynqsim(&[
        "run",
        "--project",
        &project,
        "--for",
        "1ms",
        "--break",
        &loc,
    ]);
    assert!(o.status.success());
    let text = stdout(&o);
    assert!(
        text.lines()
            .any(|l| l.starts_with("stopped") && l.contains(&format!("main.rs:{line}"))),
        "{text}"
    );
}

#[test]
fn break_needs_a_project_and_a_known_location() {
    let o = zynqsim(&["run", "--for", "1ms", "--break", "main.rs:1"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--break needs --project"));
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let project = root().join("examples/rust-debug/project.toml");
    let o = zynqsim(&[
        "run",
        "--project",
        &project.to_string_lossy(),
        "--for",
        "1ms",
        "--break",
        "nosuch.rs:9",
    ]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("no source file `nosuch.rs`"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
