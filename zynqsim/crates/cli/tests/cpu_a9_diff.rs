//! Differential test of the native `cpu-a9` interpreter against the Xilinx QEMU backend: the
//! same `rust_hello.elf` (the `examples/rust-hello` project) runs in QEMU via `zynqsim run` and
//! in the interpreter over a flat RAM with a stub UART1, and the UART text must be identical.
//! Skips with `SKIPPED:` when the Xilinx QEMU is not installed.

use std::path::PathBuf;
use std::process::{Command, Output};

use cpu_a9::{Cpu, StepResult};
use ps_bus::{Bus, BusResult, RamBus};
use serde_json::Value;

const UART1: u32 = 0xE000_1000;
const UART_FIFO: u32 = 0x30;

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn qemu_found() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).expect("inspect prints JSON");
    v["backends"]
        .as_array()
        .expect("backends list")
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

/// RAM at the firmware's link base, plus a UART1 whose TX FIFO writes are captured. Every other
/// device address reads as zero and ignores writes.
struct StubBus {
    ram: RamBus,
    uart: Vec<u8>,
}

impl Bus for StubBus {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        if (u64::from(UART1)..u64::from(UART1) + 0x1000).contains(&addr) {
            return Ok(0);
        }
        self.ram.read(addr, size)
    }

    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        if addr == u64::from(UART1 + UART_FIFO) {
            self.uart.push(value as u8);
            return Ok(());
        }
        if (u64::from(UART1)..u64::from(UART1) + 0x1000).contains(&addr) {
            return Ok(());
        }
        self.ram.write(addr, size, value)
    }
}

/// Loads the PT_LOAD segments of a 32-bit little-endian ELF and returns the entry point.
fn load_elf(bytes: &[u8], ram: &mut RamBus) -> u32 {
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
    let phoff = u32_at(0x1C) as usize;
    let phentsize = usize::from(u16_at(0x2A));
    for i in 0..usize::from(u16_at(0x2C)) {
        let ph = phoff + i * phentsize;
        if u32_at(ph) != 1 {
            continue;
        }
        let off = u32_at(ph + 4) as usize;
        let filesz = u32_at(ph + 16) as usize;
        ram.load(u64::from(u32_at(ph + 8)), &bytes[off..off + filesz]);
    }
    u32_at(0x18)
}

/// Runs the firmware in the interpreter until it parks in a self-loop, and returns its UART.
fn interpreter_uart(elf: &[u8]) -> String {
    let mut bus = StubBus {
        ram: RamBus::new(0x0010_0000, 0x0040_0000),
        uart: Vec::new(),
    };
    let entry = load_elf(elf, &mut bus.ram);
    let mut cpu = Cpu::new(0);
    cpu.reset(entry);
    for _ in 0..5_000_000u32 {
        let pc = cpu.pc();
        match cpu.step(&mut bus) {
            StepResult::Executed { event: None, .. } if cpu.pc() == pc => break,
            StepResult::Executed { .. } => {}
            StepResult::Unimplemented { pc, insn } => {
                panic!("interpreter: unimplemented {insn:#010x} at {pc:#010x}")
            }
        }
    }
    String::from_utf8_lossy(&bus.uart).into_owned()
}

#[test]
fn rust_hello_prints_the_same_text_in_qemu_and_in_the_interpreter() {
    if !qemu_found() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let project = root().join("examples/rust-hello/project.toml");
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        project.to_str().expect("utf-8 path"),
        "--for",
        "5ms",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).expect("run prints JSON");
    let qemu = v["uart"]["1"].as_str().expect("uart 1 text").to_string();

    let elf = std::fs::read(root().join("fixtures/elf/rust_hello.elf")).expect("fixture");
    let native = interpreter_uart(&elf);

    assert!(!native.is_empty(), "the interpreter printed nothing");
    assert_eq!(native, qemu, "QEMU and cpu-a9 UART text differ");
    eprintln!("UART (qemu == cpu-a9): {native:?}");
}
