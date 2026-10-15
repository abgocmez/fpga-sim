//! A QEMU session with an SD card and QSPI flash attached: what the status
//! says, and which session features hold (read-only storage) or are refused
//! (writable storage) because re-execution must start from the same disk.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::storage::{SdImageOptions, build_qspi_image, build_sd_image};
use sim_api::{Capability, Command, Engine, Input, Project, Query, Response};
use std::path::{Path, PathBuf};

fn qemu_present() -> bool {
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "zynqsim-storage-session-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A card with two files and a flash with a marker at 1 MiB, plus a project
/// running `firmware` over them.
fn setup(dir: &Path, firmware: &str, extra: &str) -> PathBuf {
    let files = dir.join("card");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("HELLO.TXT"), "storage session test\n").unwrap();
    std::fs::write(
        dir.join("sd.img"),
        build_sd_image(&files, &SdImageOptions::default()).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("flash.bin"),
        build_qspi_image(b"marker", 0x10_0000, 16).unwrap(),
    )
    .unwrap();
    let elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/elf")
        .join(firmware)
        .canonicalize()
        .unwrap();
    let project = dir.join("project.toml");
    std::fs::write(
        &project,
        format!(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"qemu\"\n\
             pl = \"demo\"\n[boot]\nmode = \"elf\"\n[cpu0]\nelf = '{}'\n[storage]\n\
             sd_image = \"sd.img\"\nqspi_image = \"flash.bin\"\n{extra}",
            elf.display()
        ),
    )
    .unwrap();
    project
}

fn engine(project: &Path) -> Box<dyn Engine> {
    Project::load(project)
        .unwrap()
        .build_engine(project.parent().unwrap())
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
fn read_only_storage_keeps_snapshots_reset_and_replay_exact() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("ro");
    let project = setup(&d, "rust_sd.elf", "");
    let mut e = engine(&project);

    let s = e.status(false);
    assert!(s.capabilities.contains(&Capability::Storage));
    assert!(s.capabilities.contains(&Capability::Snapshot));
    assert!(s.capabilities.contains(&Capability::Replay));
    let st = s.storage.expect("storage status");
    let sd = st.sd.expect("sd");
    assert_eq!(
        (sd.source.as_str(), sd.bytes, sd.writable),
        ("sd.img", 8 << 20, false)
    );
    assert_eq!(sd.model, "sd-card (SDSC)");
    let q = st.qspi.expect("qspi");
    assert_eq!(
        (q.source.as_str(), q.bytes, q.model.as_str(), q.writable),
        ("flash.bin", 16 << 20, "s25fl129p0", false)
    );

    // The firmware needs about 100 us of virtual time: snapshot part way.
    run_us(e.as_mut(), 40);
    e.command(Command::SnapshotSave { name: "a".into() })
        .unwrap();
    let at_a = state(e.as_ref());
    run_us(e.as_mut(), 1000);
    let later = state(e.as_ref());
    let text = uart(e.as_mut());
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
    assert_ne!(later.2, at_a.2);

    // Restore re-executes from the same card and reproduces the snapshot.
    e.command(Command::SnapshotRestore { name: "a".into() })
        .unwrap();
    assert_eq!(state(e.as_ref()), at_a);
    run_us(e.as_mut(), 1000);
    assert_eq!(state(e.as_ref()), later, "same card, same future");
    assert_eq!(uart(e.as_mut()), text);

    // The recording replays to the same digest.
    let Response::Recording(mut rec) = e.query(&Query::Recording, false).unwrap() else {
        panic!("no recording")
    };
    rec.project = Some(project.display().to_string());
    let report = sim_api::replay_recording(&rec).unwrap_or_else(|er| panic!("{er}"));
    assert!(report.matches, "{report:?}");

    // A reset is a fresh start on the same card.
    e.command(Command::Input(Input::Reset { seed: None }))
        .unwrap();
    run_us(e.as_mut(), 40);
    run_us(e.as_mut(), 1000);
    assert_eq!(
        state(e.as_ref()),
        later,
        "a reset session repeats the original"
    );
}

#[test]
fn writable_storage_drops_snapshot_and_replay_and_says_why() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("rw");
    let project = setup(&d, "rust_sd.elf", "writable = true\n");
    let mut e = engine(&project);
    let s = e.status(false);
    assert!(s.capabilities.contains(&Capability::Storage));
    assert!(!s.capabilities.contains(&Capability::Snapshot));
    assert!(!s.capabilities.contains(&Capability::Replay));
    assert!(s.storage.unwrap().sd.unwrap().writable);

    run_us(e.as_mut(), 1000);
    assert!(uart(e.as_mut()).contains("RESULT: ALL PASS"));

    let why = |r: Result<Response, sim_api::ApiError>| -> String {
        let e = r.expect_err("must be refused");
        assert_eq!(e.code, sim_api::ErrorCode::Unsupported, "{e}");
        e.message
    };
    let m = why(e.command(Command::SnapshotSave { name: "a".into() }));
    assert!(
        m.contains("writable = true") && m.contains("snapshot"),
        "{m}"
    );
    let m = why(e.command(Command::Input(Input::Reset { seed: None })));
    assert!(m.contains("reset"), "{m}");
    let m = why(e.query(&Query::Recording, false));
    assert!(m.contains("recording"), "{m}");
    // Running and reading memory are unaffected.
    run_us(e.as_mut(), 100);
    assert!(
        e.query(
            &Query::MemRead {
                addr: 0x0010_0000,
                len: 16
            },
            false
        )
        .is_ok()
    );
}
