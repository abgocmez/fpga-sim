//! Source-level debugging of Rust firmware: a stop is reported as a line of
//! `firmware/debug_demo/src/main.rs`, from the DWARF line table of
//! `fixtures/elf/rust_debug.elf`.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{
    BreakKind, Command, DebugSection, Engine, ErrorCode, Project, Query, Response, SourceRef,
};
use std::path::PathBuf;

fn qemu_present() -> bool {
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn demo_source() -> String {
    std::fs::read_to_string(root().join("firmware/debug_demo/src/main.rs")).unwrap()
}

/// 1-based line of the `// MARK:name` comment in the demo source.
fn mark(name: &str) -> u32 {
    demo_source()
        .lines()
        .position(|l| l.contains(&format!("MARK:{name}")))
        .map(|i| i as u32 + 1)
        .unwrap_or_else(|| panic!("MARK:{name} not found"))
}

fn build(example: &str, breakpoints: Option<Vec<String>>) -> Result<Box<dyn Engine>, String> {
    let file = root().join("examples").join(example).join("project.toml");
    let mut p = Project::load(&file).unwrap();
    if let Some(b) = breakpoints {
        let d = p.debug.get_or_insert_with(DebugSection::default);
        d.enabled = true;
        d.breakpoints = b;
    }
    p.build_engine(file.parent().unwrap())
        .map_err(|e| e.message)
}

fn run_us(e: &mut dyn Engine, us: u64) {
    e.command(Command::Run {
        duration_ps: us * 1_000_000,
    })
    .unwrap();
}

fn regs(e: &mut dyn Engine) -> sim_api::CpuRegs {
    match e.query(&Query::CpuRegs { core: 0 }, false).unwrap() {
        Response::CpuRegs(r) => r,
        r => panic!("{r:?}"),
    }
}

#[test]
fn a_stop_is_reported_as_a_line_of_the_rust_source() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let line = mark("collatz-step");
    // The example's `main.rs:LINE` breakpoint must follow the source.
    let mut e = build("rust-debug", None).unwrap();
    let bps = match e.query(&Query::Breakpoints, false).unwrap() {
        Response::Breakpoints(b) => b,
        r => panic!("{r:?}"),
    };
    assert!(!bps.is_empty() && bps.iter().all(|b| b.kind == BreakKind::Exec));
    for b in &bps {
        let s = b.source.as_ref().expect("breakpoint has a source line");
        assert!(s.file.ends_with("debug_demo/src/main.rs"), "{s:?}");
        assert_eq!(
            s.line, line,
            "examples/rust-debug/project.toml must break on MARK:collatz-step"
        );
    }

    run_us(e.as_mut(), 500);
    assert!(e.halted().is_some(), "stopped at the line breakpoint");
    let r = regs(e.as_mut());
    let src = r.pc_source.clone().expect("pc_source while stopped");
    assert!(src.file.ends_with("debug_demo/src/main.rs"), "{src:?}");
    assert_eq!(src.line, line);
    assert!(bps.iter().any(|b| b.addr == u64::from(r.r[15])));
    // The stop reason names the line too.
    let reason = e.status(false).debug.unwrap().reason.unwrap();
    assert!(reason.contains(&format!("main.rs:{line}")), "{reason}");

    // SourceLine at the pc: same line, a column, the function.
    match e
        .query(&Query::SourceLine { addr: u64::from(r.r[15]) }, false)
        .unwrap()
    {
        Response::SourceLine(l) => {
            assert_eq!((l.line, l.file.as_str()), (line, src.file.as_str()));
            assert!(l.column > 0);
            assert_eq!(l.function.as_deref(), Some("collatz_steps"));
        }
        r => panic!("{r:?}"),
    }
    // The file's text, found through `[debug] source_dirs`, has the marked
    // statement on that very line.
    match e
        .query(&Query::SourceFile { path: src.file.clone() }, false)
        .unwrap()
    {
        Response::SourceFile { text, resolved, .. } => {
            assert!(resolved.ends_with("main.rs"), "{resolved}");
            let l = text.lines().nth(line as usize - 1).unwrap();
            assert!(l.contains("MARK:collatz-step"), "{l}");
        }
        r => panic!("{r:?}"),
    }

    // Continuing hits the same line again (a loop), and the line is stable.
    run_us(e.as_mut(), 500);
    assert!(e.halted().is_some());
    let again = regs(e.as_mut()).pc_source.unwrap();
    assert_eq!(again, SourceRef { file: src.file, line });
}

#[test]
fn a_source_line_breakpoint_can_be_set_at_run_time() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let line = mark("call-checksum");
    let mut e = build("rust-debug", Some(vec![])).unwrap();
    let (file, got, addrs) = match e
        .query(
            &Query::SourceAddrs {
                file: "main.rs".into(),
                line,
            },
            false,
        )
        .unwrap()
    {
        Response::SourceAddrs { file, line, addrs } => (file, line, addrs),
        r => panic!("{r:?}"),
    };
    assert_eq!(got, line);
    assert!(file.ends_with("debug_demo/src/main.rs"));
    assert!(!addrs.is_empty());
    for addr in addrs {
        e.command(Command::SetBreakpoint {
            addr,
            kind: BreakKind::Exec,
            len: 4,
            enable: true,
        })
        .unwrap();
    }
    run_us(e.as_mut(), 500);
    assert!(e.halted().is_some(), "stopped at the call-checksum line");
    let s = regs(e.as_mut()).pc_source.unwrap();
    assert_eq!(s.line, line);
    // The firmware had already printed the collatz result.
    match e
        .query(&Query::SourceFile { path: s.file }, false)
        .unwrap()
    {
        Response::SourceFile { text, .. } => assert!(text.contains("fn collatz_steps")),
        r => panic!("{r:?}"),
    }
}

#[test]
fn bad_source_locations_are_refused_with_the_reason() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let err = build("rust-debug", Some(vec!["nosuch.rs:3".into()]))
        .err()
        .expect("unknown file");
    assert!(err.contains("no source file `nosuch.rs`"), "{err}");
    // An ELF without line info says so.
    let err = build("rust-hello", Some(vec!["main.rs:3".into()]))
        .err()
        .expect("no debug info");
    assert!(err.contains("no line information"), "{err}");
    // The queries on such an ELF report the same, as `not_found`.
    let mut e = build("rust-hello", Some(vec![])).unwrap();
    let err = e
        .query(&Query::SourceLine { addr: 0x0010_0000 }, false)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("no line information"), "{}", err.message);
}
