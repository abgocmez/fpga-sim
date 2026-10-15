//! Snapshot, restore, reset and replay of a QEMU session by deterministic
//! re-execution of its journal.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{Command, Engine, Input, Project, Query, Recording, Response};
use std::path::PathBuf;

fn qemu_present() -> bool {
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn project_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/math-qemu/project.toml")
        .canonicalize()
        .unwrap()
}

fn engine() -> Box<dyn Engine> {
    let file = project_file();
    Project::load(&file)
        .unwrap()
        .build_engine(file.parent().unwrap())
        .unwrap_or_else(|e| panic!("{e}"))
}

fn state(e: &dyn Engine) -> (u64, usize, String) {
    let s = e.status(false);
    (s.now_ps, s.trace_len, s.trace_hash)
}

fn uart(e: &mut dyn Engine) -> String {
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
        Response::Uart { data, .. } => String::from_utf8_lossy(&data).into_owned(),
        r => panic!("{r:?}"),
    }
}

fn run_us(e: &mut dyn Engine, us: u64) {
    e.command(Command::Run {
        duration_ps: us * 1_000_000,
    })
    .unwrap();
}

#[test]
fn snapshot_restore_and_reset_reproduce_the_session() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine();
    run_us(e.as_mut(), 60);
    e.command(Command::SnapshotSave { name: "a".into() })
        .unwrap();
    let at_a = state(e.as_ref());
    assert!(at_a.0 >= 60_000_000);
    run_us(e.as_mut(), 400);
    let later = state(e.as_ref());
    assert_ne!(later.2, at_a.2, "the run moved on");
    let text_later = uart(e.as_mut());

    // Back to the snapshot: exactly its state, UART log included.
    e.command(Command::SnapshotRestore { name: "a".into() })
        .unwrap();
    assert_eq!(state(e.as_ref()), at_a, "restore reproduces the snapshot");
    assert!(text_later.starts_with(&uart(e.as_mut())));
    match e.query(&Query::Snapshots, false).unwrap() {
        Response::Snapshots(n) => assert_eq!(n, vec!["a".to_string()]),
        r => panic!("{r:?}"),
    }

    // Continuing from the restored machine gives the same future.
    run_us(e.as_mut(), 400);
    assert_eq!(state(e.as_ref()), later, "same inputs, same future");
    assert_eq!(uart(e.as_mut()), text_later);

    // Unknown snapshot names are an error, not a no-op.
    assert!(
        e.command(Command::SnapshotRestore {
            name: "nope".into()
        })
        .is_err()
    );

    // Reset starts over from time zero with the same firmware.
    e.command(Command::Input(Input::Reset { seed: None }))
        .unwrap();
    let z = state(e.as_ref());
    assert!(z.0 < 1_000_000, "time restarted: {} ps", z.0);
    assert!(uart(e.as_mut()).is_empty());
    match e.query(&Query::Snapshots, false).unwrap() {
        Response::Snapshots(n) => assert!(n.is_empty(), "reset drops snapshots"),
        r => panic!("{r:?}"),
    }
    run_us(e.as_mut(), 60);
    run_us(e.as_mut(), 400);
    assert_eq!(
        state(e.as_ref()),
        (later.0, later.1, later.2.clone()),
        "a reset session repeats the original"
    );
}

#[test]
fn a_recorded_session_replays_to_the_same_digest() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine();
    run_us(e.as_mut(), 200);
    e.command(Command::Step).unwrap();
    run_us(e.as_mut(), 3000);
    let end = state(e.as_ref());
    let mut rec: Recording = match e.query(&Query::Recording, false).unwrap() {
        Response::Recording(r) => *r,
        r => panic!("{r:?}"),
    };
    assert_eq!(rec.trace_hash, end.2);
    assert!(rec.journal.len() >= 3, "{:?}", rec.journal);

    // Without a project the recording cannot be replayed, and says so.
    let err = sim_api::replay_recording(&rec).unwrap_err();
    assert!(err.message.contains("project"), "{}", err.message);

    rec.project = Some(project_file().display().to_string());
    // Survives a JSON round trip, as `run --record` writes it.
    let rec: Recording = serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
    let report = sim_api::replay_recording(&rec).unwrap();
    assert!(report.matches, "{report:?}");
    assert_eq!(report.trace_hash, end.2);

    // A tampered digest is detected.
    let mut bad = rec.clone();
    bad.trace_hash = "fnv1a64:0000000000000000".to_string();
    assert!(!sim_api::replay_recording(&bad).unwrap().matches);
}
