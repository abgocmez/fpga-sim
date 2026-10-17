//! The Cortex-A9 timers' tick rate under the Xilinx QEMU (ADR-029).
//!
//! `firmware/timer_rate` prints `DONE_GT` once the global timer has counted
//! 1,000,000 ticks and `DONE_PT` once the private timer has, and the test reads
//! the simulated time at which those lines appear. The reference board's CPU_3x2x
//! clock is 333.33 MHz (3 ms for 1e6 ticks); QEMU's reset value is 100 MHz
//! (10 ms). `[ps] cpu_clock` (default `666.667MHz`) sets the global timer to the
//! BSP's rate. The private timer does not follow it in this QEMU build, and the
//! test pins that as a known deviation (docs/conformance/bsp-apps.md, D3).
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

fn qemu_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn example() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/timer-rate/project.toml")
        .to_string_lossy()
        .into_owned()
}

/// Runs `project` for `duration` and returns the UART1 text.
fn uart_after(project: &str, duration: &str) -> String {
    let o = zynqsim(&["--json", "run", "--project", project, "--for", duration]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    v["uart"]["1"].as_str().unwrap().to_string()
}

/// A project file next to the test's scratch directory with the given
/// `[ps] cpu_clock` line (or none), running the same firmware.
fn project_with_clock(name: &str, clock_line: &str) -> String {
    let elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/elf/rust_timer_rate.elf")
        .to_string_lossy()
        .replace('\\', "/");
    let dir = std::env::temp_dir().join("zynqsim-a9-timer-rate");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.toml"));
    let text = format!(
        "schema_version = 1\n\n[board]\nname = \"zybo-z7-20\"\n\n[backends]\nps = \"qemu\"\n\
         pl = \"demo\"\n\n[sim]\nquantum_ns = 1000\nicount_shift = 1\n\n[boot]\nmode = \"elf\"\n\n\
         [ps]\n{clock_line}\n\n[cpu0]\nelf = \"{elf}\"\n"
    );
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn the_global_timer_ticks_at_the_bsp_rate_by_default() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    // 1e6 ticks at 333.33 MHz take 3.0 ms: not done at 2 ms, done by 4 ms
    // (at QEMU's 100 MHz they take 10 ms).
    let early = uart_after(&example(), "2ms");
    assert!(!early.contains("DONE_GT"), "{early}");
    let late = uart_after(&example(), "4ms");
    assert!(late.contains("DONE_GT"), "{late}");
}

#[test]
fn the_reset_value_keeps_the_global_timer_at_100_mhz() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    let project = project_with_clock("reset", "cpu_clock = \"reset\"");
    let uart = uart_after(&project, "4ms");
    assert!(!uart.contains("DONE_GT"), "{uart}");
    let uart = uart_after(&project, "12ms");
    assert!(uart.contains("DONE_GT"), "{uart}");
}

#[test]
fn the_private_timer_still_ticks_at_100_mhz_a_known_deviation() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    // The global timer is done at about 3.3 ms; the private timer needs
    // another 10 ms at 100 MHz (about 6.5 ms if it followed the CPU clock).
    let uart = uart_after(&example(), "8ms");
    assert!(uart.contains("DONE_GT"), "{uart}");
    assert!(
        !uart.contains("DONE_PT"),
        "the private timer runs at the BSP rate now: update D3 in docs/conformance/bsp-apps.md and this test\n{uart}"
    );
}
