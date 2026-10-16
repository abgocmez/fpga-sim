//! AXI GPIO, AXI Timer and AXI UART Lite (vendor-ip models) under the real QEMU and the real
//! firmware `firmware/gpio_timer_uart`, through `examples/gpio-timer-uart`.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{Command, Engine, Input, Project, Query, Response};
use std::path::PathBuf;

fn qemu_present() -> bool {
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn engine() -> Box<dyn Engine> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gpio-timer-uart/project.toml");
    Project::load(&file)
        .unwrap()
        .build_engine(file.parent().unwrap())
        .unwrap_or_else(|e| panic!("{e}"))
}

fn run_us(e: &mut dyn Engine, us: u64) {
    e.command(Command::Run {
        duration_ps: us * 1_000_000,
    })
    .unwrap();
}

fn uart(e: &mut dyn Engine, port: u8) -> String {
    match e
        .query(
            &Query::UartRead {
                port,
                from: 0,
                max: 1 << 16,
            },
            false,
        )
        .unwrap()
    {
        Response::Uart { data, .. } => String::from_utf8_lossy(&data).into_owned(),
        r => panic!("{r:?}"),
    }
}

fn leds(e: &dyn Engine) -> Vec<u8> {
    e.board().leds.iter().map(|d| u8::from(*d != 0)).collect()
}

#[test]
fn the_firmware_drives_all_three_models_and_the_board_follows() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine();
    let st = e.status(false);
    assert_eq!(st.pl_backend, "native");
    assert!(st.capabilities.contains(&sim_api::Capability::BoardIo));

    // A switch is on before the firmware looks at them.
    e.command(Command::Input(Input::SetSwitch { index: 1, on: true }))
        .unwrap();
    run_us(e.as_mut(), 6_000);

    let u1 = uart(e.as_mut(), 1);
    // LED walk through the output GPIO, read back through the register.
    for i in 0..4 {
        assert!(
            u1.contains(&format!("led walk {:#x} reads {:#x}", 1 << i, 1 << i)),
            "{u1}"
        );
    }
    // Three timer interrupts, taken through GIC 61.
    assert!(u1.contains("timer ticks=3"), "{u1}");
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");
    // The input GPIO saw the switch (channel 1 bit 1).
    assert!(u1.contains("sw=0x2 btn=0x0"), "{u1}");
    // The LEDs mirror it.
    assert_eq!(leds(e.as_ref()), vec![0, 1, 0, 0]);
    // The greeting came out of the PL's UART Lite, one character time apart.
    assert_eq!(uart(e.as_mut(), 2), "Hello from AXI UART Lite\r\n");

    // The LED walk is in the trace with its times.
    let (recs, _) = match e
        .query(
            &Query::Trace {
                from: 0,
                max: 1_000_000,
            },
            false,
        )
        .unwrap()
    {
        Response::Trace { records, next, .. } => (records, next),
        r => panic!("{r:?}"),
    };
    let led_events = recs
        .iter()
        .filter(|r| {
            matches!(&r.event, sim_api::trace::TraceEvent::Signal { name, .. } if name.starts_with("board.led"))
        })
        .count();
    assert!(led_events >= 8, "walk on and off: {led_events}");

    // A button press: all LEDs on, and the GPIO interrupt count goes up.
    e.command(Command::Input(Input::SetButton {
        index: 0,
        pressed: true,
    }))
    .unwrap();
    run_us(e.as_mut(), 400);
    assert_eq!(leds(e.as_ref()), vec![1, 1, 1, 1]);
    e.command(Command::Input(Input::SetButton {
        index: 0,
        pressed: false,
    }))
    .unwrap();
    run_us(e.as_mut(), 400);
    assert_eq!(leds(e.as_ref()), vec![0, 1, 0, 0]);
    let u1 = uart(e.as_mut(), 1);
    assert!(u1.contains("btn=0x1"), "{u1}");
    // Interrupt on input change: at least the button press and release, each toggled out.
    let irqs: u32 = u1
        .lines()
        .filter_map(|l| l.rsplit("gpio_irqs=").next()?.trim().parse().ok())
        .max()
        .unwrap_or(0);
    assert!(irqs >= 2, "gpio_irqs {irqs}: {u1}");

    // Host to PL UART: the firmware echoes what the receiver got.
    e.command(Command::Input(Input::UartWrite {
        port: 2,
        data: b"ping".to_vec(),
    }))
    .unwrap();
    run_us(e.as_mut(), 3_000);
    assert_eq!(
        uart(e.as_mut(), 2),
        "Hello from AXI UART Lite\r\nping",
        "echo through the UART Lite"
    );
    // A UART that does not exist is an error with the port range in it.
    let err = e
        .command(Command::Input(Input::UartWrite {
            port: 3,
            data: vec![1],
        }))
        .unwrap_err();
    assert!(err.message.contains("no uart 3"), "{}", err.message);
}

#[test]
fn the_session_is_deterministic() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let mut e = engine();
        e.command(Command::Input(Input::SetSwitch { index: 3, on: true }))
            .unwrap();
        run_us(e.as_mut(), 4_000);
        e.command(Command::Input(Input::UartWrite {
            port: 2,
            data: b"x".to_vec(),
        }))
        .unwrap();
        run_us(e.as_mut(), 1_000);
        hashes.push((e.status(false).trace_hash, uart(e.as_mut(), 2)));
    }
    assert_eq!(hashes[0], hashes[1]);
    assert!(hashes[0].1.ends_with('x'), "{}", hashes[0].1);
}
