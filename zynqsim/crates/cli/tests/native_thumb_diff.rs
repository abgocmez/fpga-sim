//! The Thumb-2 firmware (`firmware/thumb_hello`, built for `thumbv7a-none-eabi`, committed as
//! `fixtures/elf/thumb_hello.elf`) on the native PS engine (ADR-030, ADR-033), and against the
//! Xilinx QEMU engine: the UART text must be identical. The QEMU half skips with `SKIPPED:` when
//! `qemu-system-aarch64` is not installed; the native half always runs.

use std::path::PathBuf;

use sim_api::{Engine, Project, Ps, Query, Response};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `examples/native-thumb` with its PS backend replaced.
fn engine(ps: &str) -> Box<dyn Engine> {
    let path = root().join("examples/native-thumb/project.toml");
    let mut project = Project::load(&path).expect("example project parses");
    project.backends.ps = ps.to_string();
    project
        .build_engine(path.parent().expect("project directory"))
        .unwrap_or_else(|e| panic!("native-thumb on {ps}: {}", e.message))
}

fn uart1(e: &mut dyn Engine) -> String {
    match e.query(
        &Query::UartRead {
            port: 1,
            from: 0,
            max: 1 << 20,
        },
        false,
    ) {
        Ok(Response::Uart { data, .. }) => String::from_utf8_lossy(&data).into_owned(),
        other => panic!("uart_read: {other:?}"),
    }
}

/// Runs in 5 ms slices until a complete line with `marker` is on UART1, or `limit` has passed.
fn run_until_marker(e: &mut dyn Engine, marker: &str, limit: Ps) -> String {
    let slice = Ps::from_ms(5);
    let mut elapsed = 0u64;
    loop {
        e.run_for(slice).expect("run");
        elapsed += slice.0;
        let text = uart1(e);
        let done = text.contains(marker) && text.ends_with('\n');
        if done || elapsed >= limit.0 {
            return text;
        }
    }
}

fn qemu_present() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

#[test]
fn the_thumb_firmware_passes_its_checks_on_the_native_ps() {
    let mut n = engine("native");
    // The firmware writes CR LF line ends; compare with bare LF.
    let text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(200)).replace('\r', "");
    assert!(
        text.starts_with("hello from Thumb on the simulated Cortex-A9\n"),
        "{text:?}"
    );
    assert!(!text.contains("FAIL") && !text.contains("PANIC"), "{text}");
    assert_eq!(
        text.matches("ok: ").count(),
        13,
        "every check passed: {text}"
    );
    assert!(text.ends_with("RESULT: ALL PASS\n"), "{text:?}");
}

#[test]
fn the_thumb_firmware_prints_the_same_text_on_qemu_and_natively() {
    if !qemu_present() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let mut q = engine("qemu");
    let qemu_text = run_until_marker(q.as_mut(), "RESULT:", Ps::from_ms(2000));
    let mut n = engine("native");
    let native_text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(200));
    assert!(!native_text.is_empty(), "the native PS printed nothing");
    assert_eq!(native_text, qemu_text, "UART text differs");
}
