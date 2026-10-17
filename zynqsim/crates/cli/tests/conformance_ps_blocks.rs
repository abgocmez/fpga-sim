//! The PS blocks that `[ps] extra_blocks = true` maps (ADR-024): the GPIO
//! (xgpiops) and the SPI controllers (xspips), run with the option on. The
//! default run keeps them unmapped (the N/A lists of the other conformance
//! tests). Here the exact per-check status with the option on is pinned: a
//! check that FAILs is a finding about the mapped model, not a pass, and the
//! test records it on purpose. The table is docs/conformance/gpio-timer.md and
//! docs/conformance/iic-spi-xadc.md.
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

/// Writes a project with `[ps] extra_blocks = true` that runs `elf` (a path
/// relative to `fixtures/elf`) on the PS, and returns its path.
fn opt_in_project(name: &str, elf: &str) -> String {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/elf");
    let elf_path = fixtures.join(elf).to_string_lossy().replace('\\', "/");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("project.toml");
    std::fs::write(
        &project,
        format!(
            "schema_version = 1\n\n[board]\nname = \"zybo-z7-20\"\n\n[backends]\nps = \"qemu\"\n\
             pl = \"demo\"\n\n[sim]\nquantum_ns = 1000\nicount_shift = 1\n\n[boot]\nmode = \"elf\"\n\n\
             [ps]\nextra_blocks = true\n\n[cpu0]\nelf = \"{elf_path}\"\n"
        ),
    )
    .unwrap();
    project.to_string_lossy().into_owned()
}

/// Runs the project for 50 ms and returns the `CHECK` lines as
/// `(number, status)`, with the status word (`PASS`, `FAIL`, `N/A`).
fn checks_of(project: &str) -> Vec<(u32, String)> {
    let o = zynqsim(&["--json", "run", "--project", project, "--for", "50ms"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["backend"]["ps"], "qemu");
    let text = v["uart"]["1"].as_str().unwrap();
    text.split('\n')
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("CHECK ")?;
            let (n, rest) = rest.split_once(' ')?;
            // The name may itself contain ": " (`... writes: TX_FIFO_full set:
            // FAIL`), so the status is the earliest `: PASS|FAIL|N/A` in the line.
            let status = ["PASS", "FAIL", "N/A"]
                .iter()
                .filter_map(|s| rest.find(&format!(": {s}")).map(|i| (i, *s)))
                .min()?
                .1;
            Some((n.parse().ok()?, status.to_string()))
        })
        .collect()
}

fn statuses(checks: &[(u32, String)], want: &str) -> Vec<u32> {
    checks
        .iter()
        .filter(|(_, s)| s == want)
        .map(|(n, _)| *n)
        .collect()
}

#[test]
fn gpio_mapped_model_fails_bank_geometry_and_reset_checks() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let project = opt_in_project("conf-gpiops-ps-extra", "rust_conf_gpiops.elf");
    let checks = checks_of(&project);
    assert_eq!(checks.len(), 12, "{checks:?}");
    // The ZynqMP GPIO model is mapped under the Zynq name. Its reset INT_MASK
    // / DIRM values and its bank 2 width differ from the checks: FAIL, recorded.
    assert_eq!(statuses(&checks, "FAIL"), vec![1, 7]);
    assert_eq!(statuses(&checks, "PASS"), vec![2, 3, 4, 5, 6, 8, 9, 10, 11]);
    assert_eq!(statuses(&checks, "N/A"), vec![12]);
}

#[test]
fn spi_mapped_model_passes_most_checks_and_fails_three() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let project = opt_in_project("conf-spips-ps-extra", "rust_conf_spips.elf");
    let checks = checks_of(&project);
    assert_eq!(checks.len(), 27, "{checks:?}");
    // Mapped as xlnx,zynq-spi-r1p6 -> cdns,spi-r1p6 (the Xilinx SPI model).
    // Checks 11 (SS/CS bits), 19 and 20 (TX FIFO full at 128 entries) differ
    // from what the firmware expects: FAIL, recorded, not hidden.
    assert_eq!(statuses(&checks, "FAIL"), vec![11, 19, 20]);
    assert_eq!(statuses(&checks, "N/A"), vec![6, 9, 23, 27]);
    assert_eq!(statuses(&checks, "PASS").len(), 20);
}
