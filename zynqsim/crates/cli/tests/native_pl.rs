//! The programmable logic on the native PS (ADR-035): no external tool runs these.
//!
//! * `examples/native-gpio-timer-uart`: the GPIO/timer/UART Lite firmware on the native PS
//!   with the vendor-ip models. Its UART text matches the QEMU example (QEMU is compared when
//!   it is installed; the test skips that half with `SKIPPED:` otherwise).
//! * Five engines in one process give one trace digest (determinism).
//! * Project validation: which PS/PL pairs are accepted and what is refused, with the reason.

use std::path::{Path, PathBuf};

use sim_api::{Command, Engine, Input, Project, Query, Response};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An example project with its PS and PL backends replaced, built into an engine.
fn engine(example: &str, ps: &str, pl: &str) -> Box<dyn Engine> {
    let dir = root().join("examples").join(example);
    let mut project = Project::load(&dir.join("project.toml")).expect("project parses");
    project.backends.ps = ps.to_string();
    project.backends.pl = pl.to_string();
    project
        .build_engine(&dir)
        .unwrap_or_else(|e| panic!("{example} ps={ps} pl={pl}: {}", e.message))
}

fn uart_text(e: &mut dyn Engine, port: u8) -> String {
    match e.query(
        &Query::UartRead {
            port,
            from: 0,
            max: 1 << 20,
        },
        false,
    ) {
        Ok(Response::Uart { data, .. }) => String::from_utf8_lossy(&data).into_owned(),
        other => panic!("uart_read {port}: {other:?}"),
    }
}

fn trace_hash(e: &dyn Engine) -> String {
    e.trace().hash_string()
}

/// Validation errors of a project given as TOML text, as `field: message`.
fn errors_of(text: &str) -> Vec<String> {
    let project = Project::from_toml(text).expect("parses");
    project
        .validate(Path::new(env!("CARGO_MANIFEST_DIR")))
        .into_iter()
        .filter(|d| d.severity == sim_api::Severity::Error)
        .map(|d| format!("{}: {}", d.field, d.message))
        .collect()
}

fn native_project(backends: &str, extra: &str) -> String {
    format!(
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\n{backends}\n\
         [boot]\nmode = \"elf\"\n[cpu0]\nelf = \"../../fixtures/elf/rust_hello.elf\"\n{extra}"
    )
}

#[test]
fn gpio_timer_uart_firmware_runs_on_the_native_ps_with_the_vendor_models() {
    let mut e = engine("native-gpio-timer-uart", "native", "native");
    // The firmware finishes its run in about 20 ms of virtual time; 60 ms leaves room.
    e.command(Command::Run {
        duration_ps: 60_000_000_000,
    })
    .expect("runs without QEMU or XSim");
    let uart1 = uart_text(e.as_mut(), 1);
    for line in [
        "gpio-timer-uart demo",
        "led walk 0x1 reads 0x1",
        "led walk 0x8 reads 0x8",
        "timer ticks=3",
        "uartlite greeting queued",
        "RESULT: ALL PASS",
    ] {
        assert!(uart1.contains(line), "UART1 lacks `{line}`:\n{uart1}");
    }
    assert_eq!(
        uart_text(e.as_mut(), 2),
        "Hello from AXI UART Lite\r\n",
        "the PL UART Lite (uart_read port 2) carries the greeting"
    );
    assert!(e.status(false).pl_backend == "native");
}

#[test]
fn gpio_timer_uart_on_the_native_ps_matches_the_qemu_text() {
    if std::env::var_os("ZYNQSIM_QEMU").is_none() && sim_api::locate_binary("qemu-system-aarch64").is_none() {
        println!("SKIPPED: qemu-system-aarch64 is not installed");
        return;
    }
    let mut native = engine("native-gpio-timer-uart", "native", "native");
    native
        .command(Command::Run {
            duration_ps: 60_000_000_000,
        })
        .unwrap();
    let mut qemu = engine("gpio-timer-uart", "qemu", "native");
    qemu.command(Command::Run {
        duration_ps: 6_000_000_000,
    })
    .unwrap();
    // The timer's count differs (`tcr=`: the PS clocks differ), every other line is the same.
    let strip = |s: String| {
        s.lines()
            .map(|l| {
                if l.starts_with("timer ticks=") {
                    "timer ticks=3".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        strip(uart_text(native.as_mut(), 1)),
        strip(uart_text(qemu.as_mut(), 1))
    );
    assert_eq!(
        uart_text(native.as_mut(), 2),
        uart_text(qemu.as_mut(), 2),
        "the PL UART Lite text"
    );
}

#[test]
fn the_switches_drive_the_leds_on_the_native_ps_through_the_gpio() {
    let mut e = engine("native-gpio-timer-uart", "native", "native");
    e.command(Command::Run {
        duration_ps: 60_000_000_000,
    })
    .unwrap();
    e.command(Command::Input(Input::SetSwitch {
        index: 0,
        on: true,
    }))
    .expect("the GPIO binds board.sw");
    e.command(Command::Run {
        duration_ps: 1_000_000_000,
    })
    .unwrap();
    assert!(e.board().switches[0], "the switch is set");
}

#[test]
fn a_native_run_gives_one_trace_digest_in_five_engines() {
    let hashes: Vec<String> = (0..5)
        .map(|_| {
            let mut e = engine("native-gpio-timer-uart", "native", "native");
            e.command(Command::Run {
                duration_ps: 60_000_000_000,
            })
            .unwrap();
            trace_hash(e.as_ref())
        })
        .collect();
    assert!(
        hashes.iter().all(|h| h == &hashes[0]),
        "digests differ: {hashes:?}"
    );
}

#[test]
fn the_validation_accepts_native_pl_and_refuses_remote_port_pl_with_the_reason() {
    let ok = errors_of(&native_project("ps = \"native\"\npl = \"native\"", ""));
    assert!(ok.is_empty(), "native PL on the native PS: {ok:?}");
    let demo = errors_of(&native_project("ps = \"native\"\npl = \"demo\"", ""));
    assert!(demo.is_empty(), "no PL is the PS-only case: {demo:?}");
    let rtl = errors_of(&native_project(
        "ps = \"native\"\npl = \"rtl-native\"",
        "[rtl]\nfiles = [\"../../fixtures/rtl/math/my_math_ip.v\"]\ntop = \"my_math_ip\"\n\
         [connect]\n\"my_math_ip.S_AXI\" = \"M_AXI_GP0@0x43C00000\"\n",
    ));
    assert!(rtl.is_empty(), "rtl-native on the native PS: {rtl:?}");
    for pl in ["xsim", "verilator"] {
        let e = errors_of(&native_project(&format!("ps = \"native\"\npl = \"{pl}\""), ""));
        assert!(
            e.iter()
                .any(|m| m.starts_with("backends.pl") && m.contains("remote-port")),
            "{pl} on the native PS is refused with the reason: {e:?}"
        );
    }
    let no_ps = errors_of(&native_project(
        "ps = \"demo\"\npl = \"rtl-native\"",
        "[rtl]\nfiles = [\"a.v\"]\ntop = \"t\"\n[connect]\n\"t.S_AXI\" = \"M_AXI_GP0@0x43C00000\"\n",
    ));
    assert!(
        no_ps.iter().any(|m| m.contains("needs a PS")),
        "rtl-native needs a PS that issues bus accesses: {no_ps:?}"
    );
}

#[test]
fn the_native_ps_refuses_a_dma_master_and_pmod_devices_with_the_reason() {
    let dma = errors_of(&native_project(
        "ps = \"native\"\npl = \"native\"",
        "[native]\ndma = \"axi_dma\"\n[connect]\n\"dma.S_AXI_LITE\" = \"M_AXI_GP0@0x40400000\"\n",
    ));
    assert!(
        dma.iter().any(|m| m.contains("HP/ACP master")),
        "a DMA needs a master port: {dma:?}"
    );
    let dev = errors_of(&native_project(
        "ps = \"native\"\npl = \"native\"",
        "[devices]\npmod_ja = \"tmp2\"\n",
    ));
    assert!(
        dev.iter().any(|m| m.contains("not attached to the native PS")),
        "Pmod devices are refused: {dev:?}"
    );
}
