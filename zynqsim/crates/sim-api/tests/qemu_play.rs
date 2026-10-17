//! Continuous running (`Play`/`Pause`) and live queries against the real QEMU
//! engine: the GUI's "Play" button must advance a CPU-backed simulation while
//! staying responsive, and Pause must stop it.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{Command, Event, Project, Query, Response, SimClient};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn project() -> (Project, PathBuf) {
    let file =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/math-qemu/project.toml");
    (Project::load(&file).expect("example parses"), file)
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
fn play_advances_a_qemu_system_and_pause_stops_it() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let (p, file) = project();
    let engine = p
        .build_engine(file.parent().unwrap())
        .unwrap_or_else(|e| panic!("{e}"));
    let client = SimClient::spawn_engine(engine);
    let events = client.subscribe().unwrap();

    // 1x real time: 1 ms of virtual time per wall-clock millisecond.
    client
        .command(Command::Play {
            ps_per_second: 1_000_000_000_000,
        })
        .unwrap();
    assert!(client.status().unwrap().playing);

    // Queries stay responsive while the engine is running.
    let start = Instant::now();
    let mut worst = Duration::ZERO;
    let mut last_now = 0;
    while start.elapsed() < Duration::from_millis(1500) {
        let t = Instant::now();
        let st = client.status().unwrap();
        worst = worst.max(t.elapsed());
        last_now = st.now_ps;
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        last_now > 1_000_000_000,
        "virtual time advanced: {last_now} ps"
    );
    assert!(
        worst < Duration::from_secs(2),
        "a status query waited {worst:?} behind the running engine"
    );

    client.command(Command::Pause).unwrap();
    let t0 = client.status().unwrap().now_ps;
    std::thread::sleep(Duration::from_millis(200));
    let st = client.status().unwrap();
    assert!(!st.playing);
    assert_eq!(st.now_ps, t0, "paused time must not move");

    // The firmware's output arrived through the UART log while playing.
    let mut text = String::new();
    if let Response::Uart { data, .. } = client
        .query(Query::UartRead {
            port: 1,
            from: 0,
            max: 4096,
        })
        .unwrap()
    {
        text = String::from_utf8_lossy(&data).into_owned();
    }
    assert!(text.contains("RESULT: ALL PASS"), "{text}");

    // The event stream saw both transitions.
    let mut playing = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let Event::Playing { playing: p } = ev {
            playing.push(p);
        }
    }
    assert_eq!(playing, [true, false]);
}
