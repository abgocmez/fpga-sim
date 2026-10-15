//! Breakpoints, stops and registers of a QEMU session under the debugger.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{BreakKind, Command, Engine, ErrorCode, Event, Project, Query, Response, SimClient};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn qemu_present() -> bool {
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn engine(example: &str) -> Box<dyn Engine> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(example)
        .join("project.toml");
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

fn regs(e: &mut dyn Engine, core: u8) -> sim_api::CpuRegs {
    match e.query(&Query::CpuRegs { core }, false).unwrap() {
        Response::CpuRegs(r) => r,
        r => panic!("{r:?}"),
    }
}

const XIL_OUT32: u32 = 0x0010_058c;

#[test]
fn the_guest_stops_at_a_symbol_breakpoint_and_registers_are_readable() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("math-debug");
    let st = e.status(false);
    assert!(st.capabilities.contains(&sim_api::Capability::CpuState));
    assert_eq!(st.debug.as_ref().map(|d| d.stopped), Some(false));
    // The breakpoint from the project, resolved from the ELF.
    match e.query(&Query::Breakpoints, false).unwrap() {
        Response::Breakpoints(b) => {
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].addr, u64::from(XIL_OUT32));
            assert_eq!(b[0].kind, BreakKind::Exec);
            assert_eq!(b[0].symbol.as_deref(), Some("Xil_Out32"));
        }
        r => panic!("{r:?}"),
    }

    // Registers cannot be read before the first stop.
    let err = e.query(&Query::CpuRegs { core: 0 }, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert!(err.message.contains("stopped"), "{}", err.message);

    run_us(e.as_mut(), 200);
    assert!(e.halted().is_some(), "stopped at the breakpoint");
    let st = e.status(false);
    let d = st.debug.unwrap();
    assert!(d.stopped && d.stops == 1, "{d:?}");
    assert_eq!((d.core, d.pc), (Some(0), Some(XIL_OUT32)));
    assert!(d.reason.unwrap().contains("Xil_Out32"));
    let r = regs(e.as_mut(), 0);
    assert_eq!(r.r[15], XIL_OUT32);
    assert_eq!((r.r[0], r.r[1]), (0xF800_0008, 0xDF0D));
    assert_eq!(r.mode, "sys");
    assert_eq!(r.pc_symbol.as_deref(), Some("Xil_Out32"));

    // Symbols both ways.
    match e
        .query(
            &Query::Symbol {
                query: "main".into(),
            },
            false,
        )
        .unwrap()
    {
        Response::Symbol(s) => assert_eq!((s.addr, s.size, s.offset), (0x0010_0614, 312, 0)),
        r => panic!("{r:?}"),
    }
    match e
        .query(
            &Query::Symbol {
                query: "0x10061c".into(),
            },
            false,
        )
        .unwrap()
    {
        Response::Symbol(s) => assert_eq!((s.name.as_str(), s.offset), ("main", 8)),
        r => panic!("{r:?}"),
    }
    assert!(
        e.query(
            &Query::Symbol {
                query: "nope".into()
            },
            false
        )
        .is_err()
    );

    // Running again continues to the next call.
    run_us(e.as_mut(), 200);
    let r2 = regs(e.as_mut(), 0);
    assert_eq!((r2.r[0], r2.r[1]), (0xF800_0900, 0xF));
    assert_eq!(e.status(false).debug.unwrap().stops, 2);

    // Clear it; the program runs to its end.
    e.command(Command::SetBreakpoint {
        addr: u64::from(XIL_OUT32),
        kind: BreakKind::Exec,
        len: 4,
        enable: false,
    })
    .unwrap();
    run_us(e.as_mut(), 2000);
    assert!(e.halted().is_none());
    match e.query(&Query::Breakpoints, false).unwrap() {
        Response::Breakpoints(b) => assert!(b.is_empty()),
        r => panic!("{r:?}"),
    }
    match e
        .query(
            &Query::UartRead {
                port: 1,
                from: 0,
                max: 1 << 16,
            },
            false,
        )
        .unwrap()
    {
        Response::Uart { data, .. } => {
            let t = String::from_utf8_lossy(&data);
            assert!(t.contains("RESULT: ALL PASS"), "{t}");
        }
        r => panic!("{r:?}"),
    }
    // Not stopped any more: registers are refused again.
    assert!(e.query(&Query::CpuRegs { core: 0 }, false).is_err());
}

#[test]
fn without_a_debugger_the_debug_calls_say_how_to_get_one() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("math-qemu");
    assert!(
        !e.status(false)
            .capabilities
            .contains(&sim_api::Capability::CpuState)
    );
    let err = e.query(&Query::CpuRegs { core: 0 }, false).unwrap_err();
    assert!(err.message.contains("[debug]"), "{}", err.message);
    let err = e
        .command(Command::SetBreakpoint {
            addr: 0x100000,
            kind: BreakKind::Exec,
            len: 4,
            enable: true,
        })
        .unwrap_err();
    assert!(err.message.contains("[debug]"), "{}", err.message);
}

#[test]
fn continuous_running_stops_at_a_breakpoint_instead_of_running_past_it() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let client = SimClient::spawn_engine(engine("math-debug"));
    let events = client.subscribe().unwrap();
    client
        .command(Command::Play {
            ps_per_second: 1_000_000_000_000,
        })
        .unwrap();
    let start = Instant::now();
    let mut fault = None;
    while start.elapsed() < Duration::from_secs(20) && fault.is_none() {
        while let Ok(ev) = events.try_recv() {
            if let Event::Fault { message } = ev {
                fault = Some(message);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let msg = fault.expect("a stop is reported");
    assert!(msg.contains("Xil_Out32"), "{msg}");
    let st = client.status().unwrap();
    assert!(!st.playing, "Play ended at the stop");
    assert!(st.debug.unwrap().stopped);
}
