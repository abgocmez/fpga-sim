//! End-to-end: real Xilinx QEMU running real firmware against a native PL.
//!
//! These tests need the Xilinx QEMU from a Vitis install. When it is absent
//! they print `SKIPPED:` and return (Rust has no stable runtime-skip), so look
//! for that word in `cargo test -- --nocapture` output.

use backend_qemu::{QemuConfig, QemuMachine, RunEnd, UartMode};
use pl_api::{BusResult, PlTarget, PsPort};
use pl_native::{MathIp, NativePl};
use std::path::PathBuf;
use std::time::Duration;

fn find_qemu() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_QEMU") {
        return Some(PathBuf::from(p));
    }
    let mut roots = vec![PathBuf::from(r"C:\Xilinx\Vitis")];
    if let Some(v) = std::env::var_os("XILINX_VITIS") {
        roots.push(PathBuf::from(v));
    }
    for root in roots {
        let mut dirs = vec![root.clone()];
        if let Ok(rd) = std::fs::read_dir(&root) {
            dirs.extend(rd.flatten().map(|e| e.path()));
        }
        for d in dirs {
            let q = d
                .join("data")
                .join("emulation")
                .join("qemu_win")
                .join("qemu")
                .join("qemu-system-aarch64.exe");
            if q.is_file() {
                return Some(q);
            }
        }
    }
    None
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("elf")
        .join(name)
}

fn work_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-qemu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn config(tag: &str, elf: &str) -> Option<QemuConfig> {
    let qemu = find_qemu()?;
    let dtb = QemuConfig::dtb_for(&qemu)?;
    Some(QemuConfig::new(qemu, dtb, fixture(elf), work_dir(tag)))
}

/// Records every access the PS makes to the PL, for determinism checks.
struct Recorder<P> {
    inner: P,
    log: Vec<(u64, bool, u64, Vec<u8>)>,
}

impl<P: PlTarget> PlTarget for Recorder<P> {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let r = self.inner.read(gp, addr, buf, now_ns, ps);
        self.log.push((now_ns, false, addr, buf.to_vec()));
        r
    }
    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.log.push((now_ns, true, addr, data.to_vec()));
        self.inner.write(gp, addr, data, now_ns, ps)
    }
    fn wire(&mut self, line: u32, level: bool, now_ns: u64) {
        self.inner.wire(line, level, now_ns);
    }
}

struct Outcome {
    uart: String,
    done_at_ns: u64,
    log: Vec<(u64, bool, u64, Vec<u8>)>,
    reads: u64,
    writes: u64,
}

/// Runs until the firmware prints `COSIM_DONE`, advancing 1 ms at a time.
fn run_math(cfg: &QemuConfig) -> Outcome {
    let mut m = QemuMachine::spawn(cfg).expect("QEMU starts");
    let mut pl = NativePl::new();
    pl.map(0, 0x43C0_0000, Box::new(MathIp::new()));
    let mut pl = Recorder {
        inner: pl,
        log: Vec::new(),
    };

    let mut uart = String::new();
    let mut target = 0u64;
    let done_at_ns = loop {
        target += 1_000_000;
        match m
            .run_until(target, &mut pl, Duration::from_secs(60))
            .expect("run")
        {
            RunEnd::ReachedTime => {}
            RunEnd::PeerClosed => panic!("QEMU exited early; uart so far: {uart:?}"),
        }
        uart.push_str(&String::from_utf8_lossy(&m.uart_take()));
        if uart.contains("COSIM_DONE") {
            break m.now_ns();
        }
        assert!(
            target < 2_000_000_000,
            "firmware did not finish; uart: {uart:?}"
        );
    };
    let s = m.stats();
    Outcome {
        uart,
        done_at_ns,
        log: pl.log,
        reads: s.reads,
        writes: s.writes,
    }
}

#[test]
fn firmware_passes_against_a_native_pl() {
    let Some(cfg) = config("math", "math_test.elf") else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let o = run_math(&cfg);
    assert!(
        o.uart.contains("my_math_ip co-sim test start"),
        "{}",
        o.uart
    );
    assert_eq!(o.uart.matches("PASS in=").count(), 7, "{}", o.uart);
    assert!(!o.uart.contains("FAIL"), "{}", o.uart);
    assert!(o.uart.contains("RESULT: ALL PASS"), "{}", o.uart);
    // 7 vectors x (write + 2 reads) = 21 beats to the PL, as in the prototype.
    let pl_beats = o
        .log
        .iter()
        .filter(|(_, _, a, _)| *a >= 0x43C0_0000)
        .count();
    assert_eq!(pl_beats, 21, "log: {:?}", o.log);
    assert_eq!(o.writes as usize, o.log.iter().filter(|e| e.1).count());
    assert!(o.reads >= 14);
}

#[test]
fn two_runs_are_identical_in_time_and_traffic() {
    let (Some(a), Some(b)) = (
        config("det-a", "math_test.elf"),
        config("det-b", "math_test.elf"),
    ) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let x = run_math(&a);
    let y = run_math(&b);
    assert_eq!(x.uart, y.uart);
    assert_eq!(x.log, y.log, "PL access timestamps/data diverged");
    assert_eq!(x.done_at_ns, y.done_at_ns);
}

#[test]
fn missing_files_are_reported_before_spawning() {
    let cfg = QemuConfig::new(
        PathBuf::from("no-such-qemu.exe"),
        PathBuf::from("no.dtb"),
        PathBuf::from("no.elf"),
        work_dir("missing"),
    );
    match QemuMachine::spawn(&cfg) {
        Err(backend_qemu::QemuError::MissingFile(p)) => {
            assert!(p.ends_with("no-such-qemu.exe"));
        }
        Ok(_) => panic!("must not start"),
        Err(e) => panic!("wrong error: {e}"),
    }
}

#[test]
fn tcp_uart_mode_also_works_and_file_mode_rejects_input() {
    let Some(mut cfg) = config("tcp", "math_test.elf") else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    cfg.uart = UartMode::Tcp;
    let o = run_math(&cfg);
    assert!(o.uart.contains("RESULT: ALL PASS"), "{}", o.uart);

    let file_cfg = config("file-in", "math_test.elf").unwrap();
    let mut m = QemuMachine::spawn(&file_cfg).unwrap();
    assert!(m.uart_write(b"x").is_err());
}

/// The entry point of a little-endian ELF32 file (e_entry).
fn elf_entry(path: &std::path::Path) -> u32 {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[..4], b"\x7fELF");
    u32::from_le_bytes([b[24], b[25], b[26], b[27]])
}

#[test]
fn memory_is_readable_and_writable_while_the_machine_is_paused() {
    let Some(cfg) = config("mem", "math_test.elf") else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let entry = u64::from(elf_entry(&cfg.elf0));
    let mut m = QemuMachine::spawn(&cfg).expect("QEMU starts");
    let mut pl = NativePl::new();
    pl.map(0, 0x43C0_0000, Box::new(MathIp::new()));
    let mut uart = String::new();
    let mut target = 0u64;
    while !uart.contains("COSIM_DONE") {
        target += 1_000_000;
        m.run_until(target, &mut pl, Duration::from_secs(60))
            .unwrap();
        uart.push_str(&String::from_utf8_lossy(&m.uart_take()));
        assert!(target < 2_000_000_000, "{uart}");
    }

    // The loaded image is there: the entry point holds real code, and reads
    // are repeatable while the machine is paused.
    let code = m.read_memory(entry, 64).expect("read at the entry point");
    assert!(
        code.iter().any(|&b| b != 0),
        "entry {entry:#x} reads as zeros"
    );
    assert_eq!(code, m.read_memory(entry, 64).unwrap());

    // Write then read back, across a chunk boundary (4 KiB) and a page.
    let scratch = 0x0200_0000u64;
    let pattern: Vec<u8> = (0..9000u32).map(|i| (i * 7 + 3) as u8).collect();
    m.write_memory(scratch, &pattern).expect("write");
    assert_eq!(m.read_memory(scratch, pattern.len()).unwrap(), pattern);
    assert!(m.read_memory(0, 0).unwrap().is_empty());
}
