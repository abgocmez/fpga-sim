//! The Verilator backend end to end: real Xilinx QEMU runs the math firmware, the PL is the
//! prototype's `my_math_ip` in a Verilator model linked with the Rust DPI library.
//!
//! The same firmware and PL run as XSim (a copy of the project with `pl = "xsim"`, when Vivado
//! is installed), and the two results are compared: the firmware's output and the number of
//! bus accesses must match. The trace digests are printed; whether they match is reported, not
//! asserted, because the two simulators are not expected to agree on everything.
//!
//! Needs QEMU and Verilator (`ZYNQSIM_VERILATOR`, `VERILATOR_ROOT`, `PATH`, or the unpacked
//! archive under `<temp>/zs-tools/verilator/oss-cad-suite`). Without them each test prints
//! `SKIPPED:` and returns.

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

/// `inspect` reports a backend as found when its program is located.
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

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds the freestanding DPI library if the working tree does not have it.
fn ensure_dpi_lib() {
    let lib = workspace().join("target/dpi/release/libxsim_dpi.a");
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
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

fn run(project: &str, extra: &[&str]) -> Value {
    let mut args = vec!["--json", "run", "--project", project, "--for", "3ms"];
    args.extend_from_slice(extra);
    let o = zynqsim(&args);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_str(&stdout(&o)).unwrap()
}

fn verilator_project() -> String {
    workspace()
        .join("examples/math-verilator/project.toml")
        .to_string_lossy()
        .into_owned()
}

/// The verilator project with `pl = "xsim"` and the fixtures' absolute paths, in `dir`.
fn xsim_twin(dir: &PathBuf) -> String {
    let fixtures = workspace()
        .join("fixtures")
        .to_string_lossy()
        .replace('\\', "/");
    let text = std::fs::read_to_string(verilator_project())
        .unwrap()
        .replace("pl = \"verilator\"", "pl = \"xsim\"")
        .replace("../../fixtures", &fixtures);
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("project.toml");
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

fn check_firmware(v: &Value) {
    let uart = v["uart"]["1"].as_str().unwrap();
    assert_eq!(uart.matches("PASS in=").count(), 7, "{uart}");
    assert!(
        uart.contains("RESULT: ALL PASS") && uart.contains("COSIM_DONE"),
        "{uart}"
    );
    assert!(!uart.contains("FAIL"), "{uart}");
}

#[test]
fn math_firmware_passes_through_verilator_and_matches_xsim() {
    if !backend_found("qemu") {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    if !backend_found("verilator") {
        eprintln!(
            "SKIPPED: Verilator not found (ZYNQSIM_VERILATOR, VERILATOR_ROOT, PATH, or unpack \
             the oss-cad-suite archive to <temp>/zs-tools/verilator/oss-cad-suite)"
        );
        return;
    }
    ensure_dpi_lib();
    let project = verilator_project();

    let first = run(&project, &[]);
    assert_eq!(first["backend"]["pl"], "verilator");
    assert_eq!(first["backend"]["ps"], "qemu");
    check_firmware(&first);
    assert_eq!(
        first["bus_accesses"], 21,
        "the same bus traffic as the native model"
    );
    // The same project again: the second run takes the cached executable.
    let again = run(&project, &[]);
    assert_eq!(again["trace_hash"], first["trace_hash"]);
    assert_eq!(again["now_ps"], first["now_ps"]);

    if !backend_found("xsim") {
        eprintln!("SKIPPED: the XSim comparison (Vivado not found)");
        return;
    }
    let twin_dir =
        std::env::temp_dir().join(format!("zynqsim-verilator-twin-{}", std::process::id()));
    let xsim = run(&xsim_twin(&twin_dir), &[]);
    assert_eq!(xsim["backend"]["pl"], "xsim");
    check_firmware(&xsim);
    assert_eq!(
        xsim["uart"]["1"], first["uart"]["1"],
        "the firmware prints the same"
    );
    assert_eq!(xsim["bus_accesses"], first["bus_accesses"]);
    eprintln!(
        "verilator: trace {} len {} now {} ps; xsim: trace {} len {} now {} ps; identical trace: {}",
        first["trace_hash"],
        first["trace_len"],
        first["now_ps"],
        xsim["trace_hash"],
        xsim["trace_len"],
        xsim["now_ps"],
        xsim["trace_hash"] == first["trace_hash"]
    );
    let _ = std::fs::remove_dir_all(&twin_dir);
}
