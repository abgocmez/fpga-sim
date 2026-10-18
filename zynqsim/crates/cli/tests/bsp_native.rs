//! Integration tests verifying unmodified Xilinx standalone-BSP applications on the native PS
//! engine (backends.ps = "native").

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RUN_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);

fn zynqsim(args: &[&str]) -> Output {
    let child = Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("zynqsim starts");
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(RUN_LIMIT) {
        Ok(out) => out.expect("zynqsim runs"),
        Err(_) => {
            if cfg!(windows) {
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
            } else {
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
            }
            panic!(
                "`zynqsim {}` did not finish within {RUN_LIMIT:?}",
                args.join(" ")
            );
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bsp_elf(name: &str) -> PathBuf {
    let p = root().join("fixtures/bsp").join(format!("{name}.elf"));
    assert!(p.is_file(), "missing {}", p.display());
    p
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-bsp-native-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Run {
    uart1: String,
}

fn run_native_app(elf_name: &str, duration: &str) -> Run {
    let elf = bsp_elf(elf_name);
    let dir = temp(elf_name);
    let toml = format!(
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"native\"\npl = \"demo\"\n\
         [sim]\nquantum_ns = 1000\n[boot]\nmode = \"elf\"\n[cpu0]\nelf = \"{}\"\n",
        elf.to_string_lossy().replace('\\', "/")
    );
    let p = dir.join("project.toml");
    std::fs::write(&p, toml).unwrap();

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &p.to_string_lossy(),
        "--for",
        duration,
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "exit non-zero for {elf_name}: stdout={}, stderr={}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_slice(&o.stdout).expect("valid JSON");
    Run {
        uart1: v["uart"]["1"].as_str().unwrap_or("").replace('\r', ""),
    }
}

#[test]
fn native_bsp_hello_world() {
    let r = run_native_app("bsp_hello_world", "15ms");
    assert!(r.uart1.contains("Hello World"));
    assert!(r.uart1.contains("Successfully ran Hello World application"));
}

#[test]
fn native_bsp_uartps_polled() {
    let r = run_native_app("bsp_uartps_polled", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran UART Polled Mode Example Test")
    );
}

#[test]
fn native_bsp_uartps_intr() {
    let r = run_native_app("bsp_uartps_intr", "25ms");
    assert!(
        r.uart1
            .contains("Successfully ran UART Interrupt Example Test")
    );
}

#[test]
fn native_bsp_uartps_selftest() {
    let r = run_native_app("bsp_uartps_selftest", "15ms");
    assert!(r.uart1.contains("Successfully ran UART Selftest Example"));
}

#[test]
fn native_bsp_scugic() {
    let r = run_native_app("bsp_scugic", "15ms");
    assert!(r.uart1.contains("Successfully ran GIC Example Test"));
}

#[test]
fn native_bsp_scugic_lowlevel() {
    let r = run_native_app("bsp_scugic_lowlevel", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran Low Level GIC Example Test")
    );
}

#[test]
fn native_bsp_scutimer_intr() {
    let r = run_native_app("bsp_scutimer_intr", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran SCU Timer Interrupt Example Test")
    );
}

#[test]
fn native_bsp_scutimer_polled() {
    let r = run_native_app("bsp_scutimer_polled", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran SCU Timer Polled Mode Example Test")
    );
}

#[test]
fn native_bsp_ttcps_lowlevel() {
    let r = run_native_app("bsp_ttcps_lowlevel", "300ms");
    assert!(
        r.uart1
            .contains("Successfully ran TTC Lowlevel Example Test")
    );
}

#[test]
fn native_bsp_ttcps_tapp() {
    let r = run_native_app("bsp_ttcps_tapp", "60ms");
    assert!(r.uart1.contains("Successfully ran ttcps tapp Example"));
}

#[test]
fn native_bsp_canps_polled() {
    let r = run_native_app("bsp_canps_polled", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran CAN Polled Mode Example Test")
    );
}

#[test]
fn native_bsp_iicps_selftest() {
    let r = run_native_app("bsp_iicps_selftest", "15ms");
    assert!(
        r.uart1
            .contains("Successfully ran IIC Self Test Example Test")
    );
}

#[test]
fn native_bsp_spips_selftest() {
    let r = run_native_app("bsp_spips_selftest", "15ms");
    assert!(r.uart1.contains("Successfully ran SPIPS Selftest Example"));
}

#[test]
fn native_bsp_devcfg_selftest() {
    let r = run_native_app("bsp_devcfg_selftest", "15ms");
    assert!(r.uart1.contains("Successfully ran DevCfg Selftest Example"));
}

#[test]
fn native_bsp_peripheral_tests() {
    let r = run_native_app("bsp_peripheral_tests", "30ms");
    assert!(r.uart1.contains("ScuGicSelfTestExample PASSED"));
    assert!(r.uart1.contains("CanPsPolledExample PASSED"));
}

#[test]
fn native_bsp_xadcps_polled() {
    let r = run_native_app("bsp_xadcps_polled", "60ms");
    assert!(
        r.uart1
            .contains("Successfully ran adcps polled printf Example")
    );
    assert!(
        r.uart1
            .contains("The Current Temperature is 24.979 Centigrades.")
    );
}
