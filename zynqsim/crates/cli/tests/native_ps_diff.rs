//! Differential tests of the native PS engine (`backends.ps = "native"`, ADR-033) against the
//! Xilinx QEMU engine: the same project runs on both, and the UART text and the final status
//! must agree. Where they do not, the difference is pinned here with its reason (see
//! `docs/conformance/native-ps.md`); an unexplained difference fails the test.
//!
//! The QEMU halves skip with `SKIPPED:` when `qemu-system-aarch64` is not installed. The native
//! halves always run. Plus: five separate processes give one trace digest.

use std::path::PathBuf;
use std::process::{Command as Process, Output};

use serde_json::Value;
use sim_api::{Engine, Project, Ps, Query, Response, Status};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn zynqsim(args: &[&str]) -> Output {
    Process::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn qemu_present() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

/// The example's project with its PS backend replaced.
fn engine(example: &str, ps: &str) -> Box<dyn Engine> {
    let path = root().join("examples").join(example).join("project.toml");
    let mut project = Project::load(&path).expect("example project parses");
    project.backends.ps = ps.to_string();
    project
        .build_engine(path.parent().expect("project directory"))
        .unwrap_or_else(|e| panic!("{example} on {ps}: {}", e.message))
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

/// Runs in 5 ms slices until the line with `marker` is complete on UART1 (it ends in a line
/// feed) or `limit` has passed. The firmware parks after its last line, so a finished run
/// needs no more virtual time.
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

/// One conformance line: `CHECK n name: STATUS why`.
#[derive(Debug, PartialEq)]
struct Check {
    number: u32,
    status: String,
}

/// The status word of a check line: the first `: PASS`, `: FAIL` or `: N/A` after its number
/// (a check name may itself contain a colon).
fn checks(text: &str) -> Vec<Check> {
    text.split("\r\n")
        .filter_map(|l| {
            let rest = l.strip_prefix("CHECK ")?;
            let (number, tail) = rest.split_once(' ')?;
            let status = ["PASS", "FAIL", "N/A"]
                .into_iter()
                .filter_map(|s| tail.find(&format!(": {s}")).map(|i| (i, s)))
                .min()?
                .1
                .to_string();
            Some(Check {
                number: number.parse().ok()?,
                status,
            })
        })
        .collect()
}

fn non_check_lines(text: &str) -> Vec<&str> {
    text.split("\r\n")
        .filter(|l| !l.starts_with("CHECK ") && !l.starts_with("RESULT:"))
        .collect()
}

fn result_line(text: &str) -> &str {
    text.split("\r\n")
        .find(|l| l.starts_with("RESULT:"))
        .unwrap_or("(no RESULT line)")
}

/// The contract the two engines share: the same board, the same fidelity class and no PL.
fn same_contract(q: &Status, n: &Status) {
    assert_eq!(q.board, n.board, "board");
    assert_eq!(q.pl_backend, n.pl_backend, "pl backend");
    assert_eq!(q.fidelity, n.fidelity, "fidelity");
    assert_eq!(q.ps_backend, "qemu");
    assert_eq!(n.ps_backend, "native");
}

/// Compares the CHECK lines of a conformance run. Every difference must be one of `pinned`
/// (check number, QEMU status, native status), and no other check may differ.
fn assert_checks_match_except(qemu: &str, native: &str, pinned: &[(u32, &str, &str)]) {
    let q = checks(qemu);
    let n = checks(native);
    assert_eq!(q.len(), n.len(), "number of CHECK lines");
    let mut seen = Vec::new();
    for (a, b) in q.iter().zip(&n) {
        assert_eq!(a.number, b.number);
        if a.status == b.status {
            continue;
        }
        let want = pinned
            .iter()
            .find(|(no, _, _)| *no == a.number)
            .unwrap_or_else(|| {
                panic!(
                    "check {} differs: qemu {} native {}",
                    a.number, a.status, b.status
                )
            });
        assert_eq!(
            (a.status.as_str(), b.status.as_str()),
            (want.1, want.2),
            "check {} differs from the pinned difference",
            a.number
        );
        seen.push(a.number);
    }
    for (no, _, _) in pinned {
        assert!(
            seen.contains(no),
            "pinned difference for check {no} did not occur"
        );
    }
}

#[test]
fn rust_hello_prints_the_same_text_on_qemu_and_natively() {
    if !qemu_present() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let mut q = engine("rust-hello", "qemu");
    q.run_for(Ps::from_ms(20)).expect("qemu run");
    let qemu_text = uart1(q.as_mut());
    let qemu_status = q.status(false);

    let mut n = engine("native-hello", "native");
    let native_text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(200));
    let native_status = n.status(false);

    assert!(!native_text.is_empty(), "the native PS printed nothing");
    assert_eq!(native_text, qemu_text, "UART text differs");
    same_contract(&qemu_status, &native_status);
    eprintln!(
        "rust-hello: identical text ({} bytes); virtual time qemu {} ps, native {} ps",
        native_text.len(),
        qemu_status.now_ps,
        native_status.now_ps
    );
}

#[test]
fn conf_scutimer_matches_qemu_except_the_three_checks_qemu_cannot_pass() {
    if !qemu_present() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let mut q = engine("conf-scutimer", "qemu");
    q.run_for(Ps::from_ms(200)).expect("qemu run");
    let qemu_text = uart1(q.as_mut());

    let mut n = engine("conf-scutimer", "native");
    let native_text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(400));

    // Pinned (docs/conformance/native-ps.md): QEMU's global timer deviates (it ignores COUNTER
    // writes, keeps counting while disabled, and re-asserts the comparator flag after W1C); the
    // firmware reports those three checks N/A. The native model does not reproduce the
    // deviations, so the same checks PASS.
    assert_checks_match_except(
        &qemu_text,
        &native_text,
        &[
            (13, "N/A", "PASS"),
            (14, "N/A", "PASS"),
            (15, "N/A", "PASS"),
        ],
    );
    assert_eq!(non_check_lines(&native_text), non_check_lines(&qemu_text));
    assert_eq!(result_line(&qemu_text), "RESULT: ALL PASS");
    assert_eq!(result_line(&native_text), "RESULT: ALL PASS");
}

#[test]
fn conf_ttc_matches_qemu_except_the_pinned_differences() {
    if !qemu_present() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let mut q = engine("conf-ttc", "qemu");
    q.run_for(Ps::from_ms(200)).expect("qemu run");
    let qemu_text = uart1(q.as_mut());

    let mut n = engine("conf-ttc", "native");
    let native_text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(400));

    // Pinned (docs/conformance/native-ps.md):
    // * check 9: the firmware starts the decrement counter from the value check 8 left in it
    //   (it does not reset the counter), so the result depends on the instruction timing of
    //   the run. The native timing (one instruction per 1.5 ns) leaves 1009 ticks, and the
    //   two reads 14 us apart wrap the counter (209, then 64145). QEMU's timing passes. The
    //   UART back-pressure is not the cause: a 100 Mbaud UART gives the same numbers.
    // * check 12: both QEMU and the native model clear the ISR on read (matching silicon and
    //   unmodified Xilinx BSP drivers), so both report N/A.
    assert_checks_match_except(&qemu_text, &native_text, &[(9, "PASS", "FAIL")]);
    assert_eq!(non_check_lines(&native_text), non_check_lines(&qemu_text));
    assert_eq!(result_line(&qemu_text), "RESULT: ALL PASS");
    assert_eq!(
        result_line(&native_text),
        "RESULT: FAIL (1 checks)",
        "the one pinned failure is check 9"
    );
    assert!(
        checks(&native_text)
            .iter()
            .any(|c| c.number == 9 && c.status == "FAIL")
    );
}

#[test]
fn five_processes_give_one_trace_digest() {
    let digests: Vec<String> = (0..5)
        .map(|_| {
            let o = zynqsim(&[
                "--json",
                "run",
                "--project",
                root()
                    .join("examples/native-hello/project.toml")
                    .to_str()
                    .expect("utf-8 path"),
                "--for",
                "10ms",
            ]);
            assert_eq!(
                o.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&o.stderr)
            );
            let v: Value = serde_json::from_slice(&o.stdout).expect("run prints JSON");
            assert_eq!(v["backend"]["ps"], "native");
            assert_eq!(v["backend"]["fidelity"], "f0");
            v["trace_hash"].as_str().expect("trace hash").to_string()
        })
        .collect();
    assert!(digests.iter().all(|d| *d == digests[0]), "{digests:?}");
}

#[test]
fn inspect_lists_the_native_ps_as_available() {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_slice(&o.stdout).expect("inspect prints JSON");
    let native = v["backends"]
        .as_array()
        .expect("backends list")
        .iter()
        .find(|b| b["name"] == "native" && b["role"] == "ps")
        .expect("native PS listed");
    assert_eq!(native["implemented"], true);
}

#[test]
fn a_native_project_with_a_second_core_is_accepted() {
    let base = root().join("examples/dual-core-native");
    let p = Project::load(&base.join("project.toml")).expect("project");
    assert!(p.cpu1.is_some());
    let errors: Vec<String> = p
        .validate(&base)
        .into_iter()
        .filter(|d| d.severity == sim_api::Severity::Error)
        .map(|d| format!("{}: {}", d.field, d.message))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

fn uart_port(e: &mut dyn Engine, port: u8) -> String {
    match e.query(
        &Query::UartRead {
            port,
            from: 0,
            max: 1 << 20,
        },
        false,
    ) {
        Ok(Response::Uart { data, .. }) => String::from_utf8_lossy(&data).into_owned(),
        other => panic!("uart_read: {other:?}"),
    }
}

/// The dual-core demo on both engines. UART1 (core 0's console) must be identical and end in
/// `RESULT: ALL PASS`. UART0 (core 1's console) is only observable natively: the QEMU engine
/// wires UART0 to `-serial null`, so that difference is pinned here, not hidden.
#[test]
fn dual_core_demo_prints_the_same_uart1_text_on_qemu_and_natively() {
    let mut n = engine("dual-core-native", "native");
    let native_text = run_until_marker(n.as_mut(), "RESULT:", Ps::from_ms(200));
    let native_uart0 = uart_port(n.as_mut(), 0);
    assert!(
        native_text.ends_with("RESULT: ALL PASS\r\n"),
        "{native_text:?}"
    );
    assert!(
        native_uart0.contains("core 1 (MPIDR 1): released through 0xFFFFFFF0")
            && native_uart0.contains("core 1: SGI 1 received, ping-pong done"),
        "core 1 printed on UART0: {native_uart0:?}"
    );
    if !qemu_present() {
        eprintln!("SKIPPED: the QEMU half needs the Xilinx QEMU");
        return;
    }
    let mut q = engine("dual-core-qemu", "qemu");
    let qemu_text = run_until_marker(q.as_mut(), "RESULT:", Ps::from_ms(200));
    assert_eq!(
        native_text, qemu_text,
        "UART1 text differs between the engines"
    );
    assert_eq!(
        uart_port(q.as_mut(), 0),
        "",
        "pinned difference: QEMU's UART0 is `-serial null`, so core 1's lines are not seen"
    );
    same_contract(&q.status(false), &n.status(false));
}

/// Five separate processes give one trace digest for the dual-core demo.
#[test]
fn dual_core_five_processes_give_one_trace_digest() {
    let digests: Vec<String> = (0..5)
        .map(|_| {
            let o = zynqsim(&[
                "--json",
                "run",
                "--project",
                root()
                    .join("examples/dual-core-native/project.toml")
                    .to_str()
                    .expect("utf-8 path"),
                "--for",
                "80ms",
            ]);
            assert_eq!(
                o.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&o.stderr)
            );
            let v: Value = serde_json::from_slice(&o.stdout).expect("run prints JSON");
            v["trace_hash"].as_str().expect("trace hash").to_string()
        })
        .collect();
    assert!(digests.iter().all(|d| *d == digests[0]), "{digests:?}");
}
