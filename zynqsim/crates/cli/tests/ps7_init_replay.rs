//! Tests for `ps7_init` replay in both Native PS and QEMU (Task A / ADR-038).

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-ps7-init-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn ps7_init_replays_and_configures_slcr_natively() {
    let d = temp("native-replay");
    let elf = root()
        .join("fixtures/elf/rust_hello.elf")
        .to_string_lossy()
        .replace('\\', "/");

    let tcl = r#"
        proc ps7_pll_init_data_3_0 {} {
            mwr -force 0XF8000008 0x0000DF0D
            mask_write 0XF8000100 0x0007F000 0x00028000
            mask_poll 0XF800010C 0x00000001
        }
    "#;
    std::fs::write(d.join("ps7_init.tcl"), tcl).unwrap();

    let project_toml = format!(
        r#"
schema_version = 1

[board]
name = "zybo-z7-20"

[backends]
ps = "native"
pl = "demo"

[sim]
quantum_ns = 1000
seed = 1

[boot]
mode = "elf"
ps7_init = true

[cpu0]
elf = "{elf}"
"#
    );
    std::fs::write(d.join("project.toml"), project_toml).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "5ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v["uart"]["1"]
            .as_str()
            .unwrap()
            .contains("hello from Rust on the simulated Cortex-A9")
    );
}

#[test]
fn ps7_init_poll_timeout_reports_failing_register_and_mask() {
    let d = temp("poll-timeout");
    let elf = root()
        .join("fixtures/elf/rust_hello.elf")
        .to_string_lossy()
        .replace('\\', "/");

    // mask_poll on LOCKSTA (0xF800000C) for bit 1, which never gets set.
    let tcl = r#"
        proc ps7_pll_init_data_3_0 {} {
            mask_poll 0XF800000C 0x00000002
        }
    "#;
    std::fs::write(d.join("ps7_init.tcl"), tcl).unwrap();

    let project_toml = format!(
        r#"
schema_version = 1

[board]
name = "zybo-z7-20"

[backends]
ps = "native"
pl = "demo"

[sim]
quantum_ns = 1000
seed = 1

[boot]
mode = "elf"
ps7_init = true

[cpu0]
elf = "{elf}"
"#
    );
    std::fs::write(d.join("project.toml"), project_toml).unwrap();

    let o = zynqsim(&[
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "1ms",
    ]);
    assert_ne!(o.status.code(), Some(0), "run should fail on poll timeout");
    let err = stderr(&o);
    assert!(
        err.contains("0xf800000c") || err.contains("0xF800000C"),
        "error should name failing register, got:\n{err}"
    );
    assert!(
        err.contains("mask_poll timeout"),
        "error should mention timeout, got:\n{err}"
    );
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

#[test]
fn ps7_init_replays_and_configures_slcr_on_qemu() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    let d = temp("qemu-replay");
    let elf = root()
        .join("fixtures/elf/rust_hello.elf")
        .to_string_lossy()
        .replace('\\', "/");

    let tcl = r#"
        proc ps7_pll_init_data_3_0 {} {
            mwr -force 0XF8000008 0x0000DF0D
            mask_write 0XF8000100 0x0007F000 0x00028000
            mask_poll 0XF800010C 0x00000001
        }
    "#;
    std::fs::write(d.join("ps7_init.tcl"), tcl).unwrap();

    let project_toml = format!(
        r#"
schema_version = 1

[board]
name = "zybo-z7-20"

[backends]
ps = "qemu"
pl = "demo"

[sim]
quantum_ns = 1000
icount_shift = 1

[boot]
mode = "elf"
ps7_init = true

[cpu0]
elf = "{elf}"
"#
    );
    std::fs::write(d.join("project.toml"), project_toml).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "5ms",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v["uart"]["1"]
            .as_str()
            .unwrap()
            .contains("hello from Rust on the simulated Cortex-A9")
    );
}

#[test]
fn ps7_init_qemu_replays_with_explicit_allow_list_skips() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    let d = temp("qemu-allow-list");
    let elf = root()
        .join("fixtures/elf/rust_hello.elf")
        .to_string_lossy()
        .replace('\\', "/");

    // Operations touching SLCR, DDRC (allow-listed), and DEVCFG (allow-listed).
    let tcl = r#"
        proc ps7_pll_init_data_3_0 {} {
            mwr -force 0XF8000008 0x0000DF0D
            mask_write 0XF8000100 0x0007F000 0x00028000
            mask_poll 0XF800010C 0x00000001
        }
        proc ps7_ddr_init_data_3_0 {} {
            mwr 0XF8006000 0x00000001
            mask_poll 0XF8006054 0x00000007
        }
        proc ps7_peripherals_init_data_3_0 {} {
            mwr 0XF8007000 0x00000001
            mask_poll 0XF8000B74 0x00002000
        }
    "#;
    std::fs::write(d.join("ps7_init.tcl"), tcl).unwrap();

    let project_toml = format!(
        r#"
schema_version = 1

[board]
name = "zybo-z7-20"

[backends]
ps = "qemu"
pl = "demo"

[sim]
quantum_ns = 1000
icount_shift = 1

[boot]
mode = "elf"
ps7_init = true

[cpu0]
elf = "{elf}"
"#
    );
    std::fs::write(d.join("project.toml"), project_toml).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "5ms",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "replay with allow-listed registers should succeed: {}\n{}",
        stdout(&o),
        stderr(&o)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v["uart"]["1"]
            .as_str()
            .unwrap()
            .contains("hello from Rust on the simulated Cortex-A9")
    );
}

#[test]
fn ps7_init_qemu_fails_on_unallowed_register_instead_of_silently_ignoring() {
    if !qemu_available() {
        eprintln!("SKIPPED: the Xilinx QEMU is not installed");
        return;
    }
    let d = temp("qemu-unallowed");
    let elf = root()
        .join("fixtures/elf/rust_hello.elf")
        .to_string_lossy()
        .replace('\\', "/");

    // Write to 0xEA000000 (unmapped and NOT in the allow-list)
    let tcl = r#"
        proc ps7_pll_init_data_3_0 {} {
            mwr 0XEA000000 0x12345678
        }
    "#;
    std::fs::write(d.join("ps7_init.tcl"), tcl).unwrap();

    let project_toml = format!(
        r#"
schema_version = 1

[board]
name = "zybo-z7-20"

[backends]
ps = "qemu"
pl = "demo"

[sim]
quantum_ns = 1000
icount_shift = 1

[boot]
mode = "elf"
ps7_init = true

[cpu0]
elf = "{elf}"
"#
    );
    std::fs::write(d.join("project.toml"), project_toml).unwrap();

    let o = zynqsim(&[
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "1ms",
    ]);
    assert_ne!(
        o.status.code(),
        Some(0),
        "unallowed register access should cause run to fail"
    );
    let err = stderr(&o);
    assert!(
        err.contains("0xea000000") || err.contains("0XEA000000") || err.contains("allow-list"),
        "error should report unallowed register, got:\n{err}"
    );
}
