//! Shared test helpers: a flat-RAM bus with the Zynq device windows the firmware touches, a
//! minimal ELF32 loader for the prebuilt firmware, and a run loop that classifies the outcome.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::BTreeSet;
use std::path::PathBuf;

use cpu_a9::{Cpu, Event, StepResult};
use ps_bus::{Bus, BusResult, RamBus};

/// Where the firmware links (`firmware/rt/link.ld`): 4 MiB of DDR, stack at its top.
pub const RAM_BASE: u32 = 0x0010_0000;
/// Length of the firmware RAM.
pub const RAM_LEN: usize = 0x0040_0000;
/// UART1 base (the console the firmware prints on).
pub const UART1: u32 = 0xE000_1000;
/// UART1 TX FIFO register offset.
pub const UART_FIFO: u32 = 0x30;
/// The end-of-test write the testbench watches for (`firmware/rt` `finish`).
pub const FINISH_ADDR: u32 = 0x43C0_003C;
/// The value written with [`FINISH_ADDR`].
pub const FINISH_VALUE: u64 = 0xC051_D0DE;

/// A bus for the firmware: RAM, a stub UART1 that records TX bytes, and device windows that
/// read as zero and ignore writes. Records which device windows were touched.
pub struct FwBus {
    /// The RAM at [`RAM_BASE`].
    pub ram: RamBus,
    /// Bytes written to UART1 TX.
    pub uart: Vec<u8>,
    /// Device windows touched by the program.
    pub touched: BTreeSet<&'static str>,
    /// Addresses that hit no device and no RAM (first few, unique).
    pub unmapped: Vec<u64>,
    /// Set when the firmware writes the end-of-test marker.
    pub finished: bool,
}

impl FwBus {
    /// An empty bus with RAM at [`RAM_BASE`].
    pub fn new() -> FwBus {
        FwBus {
            ram: RamBus::new(u64::from(RAM_BASE), RAM_LEN),
            uart: Vec::new(),
            touched: BTreeSet::new(),
            unmapped: Vec::new(),
            finished: false,
        }
    }

    /// The name of the device window `addr` falls in, if it is one.
    fn region(addr: u64) -> Option<&'static str> {
        Some(match addr {
            0x4000_0000..=0x7FFF_FFFF => "pl-gp0",
            0xE000_1000..=0xE000_1FFF => "uart1",
            0xE000_0000..=0xE000_0FFF | 0xE000_2000..=0xEFFF_FFFF => "other-io",
            0xF800_0000..=0xF800_0FFF => "slcr",
            0xF800_1000..=0xF800_1FFF => "ttc",
            0xF800_2000..=0xF8EF_FFFF => "devcfg-and-peripherals",
            0xF8F0_0000..=0xF8F0_00FF => "scu",
            0xF8F0_0100..=0xF8F0_05FF => "gic-cpu-interface",
            0xF8F0_0600..=0xF8F0_06FF => "private-timer-watchdog",
            0xF8F0_0700..=0xF8F0_0FFF => "scu-and-gic-misc",
            0xF8F0_1000..=0xF8F0_1FFF => "gic-distributor",
            0xF8F0_2000..=0xFFFF_FFFF => "other-io",
            _ => return None,
        })
    }
}

impl Default for FwBus {
    fn default() -> Self {
        Self::new()
    }
}

impl FwBus {
    fn note_unmapped(&mut self, addr: u64) {
        if self.unmapped.len() < 8 && !self.unmapped.contains(&addr) {
            self.unmapped.push(addr);
        }
    }
}

impl Bus for FwBus {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        if let Some(r) = Self::region(addr) {
            self.touched.insert(r);
            return Ok(0);
        }
        match self.ram.read(addr, size) {
            Ok(v) => Ok(v),
            Err(e) => {
                self.note_unmapped(addr);
                Err(e)
            }
        }
    }

    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        if let Some(r) = Self::region(addr) {
            self.touched.insert(r);
            if addr == u64::from(UART1 + UART_FIFO) {
                self.uart.push(value as u8);
            }
            if addr == u64::from(FINISH_ADDR) && value == FINISH_VALUE {
                self.finished = true;
            }
            return Ok(());
        }
        match self.ram.write(addr, size, value) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.note_unmapped(addr);
                Err(e)
            }
        }
    }
}

/// The absolute path of a repository fixture, from this crate's manifest directory.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("elf")
        .join(name)
}

/// The loadable segments and entry point of a 32-bit little-endian ELF.
pub struct Elf {
    /// The entry address (`e_entry`).
    pub entry: u32,
    /// `(vaddr, bytes)` for every PT_LOAD segment (file bytes; memsz zero fill is implicit).
    pub segments: Vec<(u32, Vec<u8>)>,
}

/// Parses the ELF32 program headers of `bytes`. Panics on a malformed file (test input).
pub fn parse_elf(bytes: &[u8]) -> Elf {
    assert_eq!(&bytes[0..4], b"\x7fELF", "not an ELF file");
    assert_eq!(bytes[4], 1, "not ELF32");
    assert_eq!(bytes[5], 1, "not little endian");
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let entry = u32_at(0x18);
    let phoff = u32_at(0x1C) as usize;
    let phentsize = usize::from(u16_at(0x2A));
    let phnum = usize::from(u16_at(0x2C));
    let mut segments = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if u32_at(ph) != 1 {
            continue; // not PT_LOAD
        }
        let off = u32_at(ph + 4) as usize;
        let vaddr = u32_at(ph + 8);
        let filesz = u32_at(ph + 16) as usize;
        segments.push((vaddr, bytes[off..off + filesz].to_vec()));
    }
    Elf { entry, segments }
}

/// Why a run stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The program branched to itself (`loop {}`) with nothing executing.
    Looped { pc: u32 },
    /// The core is in WFI/WFE and no interrupt source is modelled to wake it.
    Idle { pc: u32 },
    /// The end-of-test marker was written.
    Finished,
    /// An instruction is not implemented.
    Unimplemented { pc: u32, insn: u32 },
    /// The step budget ran out.
    Budget,
}

/// The result of running one firmware image.
pub struct Run {
    /// Everything the firmware printed on UART1.
    pub uart: String,
    /// Steps executed.
    pub steps: u64,
    /// How the run ended.
    pub outcome: Outcome,
    /// Device windows the program touched.
    pub touched: Vec<&'static str>,
    /// Unmapped addresses the program touched (first few).
    pub unmapped: Vec<u64>,
    /// The first data and prefetch aborts, as `data@addr` / `prefetch@addr`.
    pub aborts: Vec<String>,
    /// The CP15 SCTLR value at the end (bit 0 is the MMU enable).
    pub sctlr: u32,
    // (see `Cpu::cp15`)
    /// The exception events seen, by name.
    pub events: Vec<&'static str>,
}

/// Loads `elf` into a fresh bus and runs it for at most `budget` steps.
pub fn run_firmware(elf: &Elf, budget: u64) -> Run {
    let mut bus = FwBus::new();
    for (vaddr, data) in &elf.segments {
        bus.ram.load(u64::from(*vaddr), data);
    }
    let mut cpu = Cpu::new(0);
    cpu.reset(elf.entry);
    let mut steps = 0u64;
    let mut events = Vec::new();
    let mut aborts: Vec<String> = Vec::new();
    let outcome = loop {
        if steps >= budget {
            break Outcome::Budget;
        }
        if bus.finished {
            break Outcome::Finished;
        }
        let pc_before = cpu.pc();
        let res = cpu.step(&mut bus);
        steps += 1;
        match res {
            StepResult::Unimplemented { pc, insn } => {
                break Outcome::Unimplemented { pc, insn };
            }
            StepResult::Executed { event, .. } => {
                if let Some(e) = event {
                    match e {
                        Event::DataAbort { addr } if aborts.len() < 4 => {
                            aborts.push(format!("data@{addr:#010x}"));
                        }
                        Event::PrefetchAbort { addr } if aborts.len() < 4 => {
                            aborts.push(format!("prefetch@{addr:#010x}"));
                        }
                        _ => {}
                    }
                    let name = match e {
                        Event::Idle => {
                            break Outcome::Idle { pc: pc_before };
                        }
                        Event::Wfi => "wfi",
                        Event::Wfe => "wfe",
                        Event::Sev => "sev",
                        Event::Svc(_) => "svc",
                        Event::Bkpt(_) => "bkpt",
                        Event::Undefined => "undefined",
                        Event::DataAbort { .. } => "data-abort",
                        Event::PrefetchAbort { .. } => "prefetch-abort",
                        Event::Irq => "irq",
                        Event::Fiq => "fiq",
                    };
                    if events.last() != Some(&name) {
                        events.push(name);
                    }
                }
                if cpu.pc() == pc_before && event.is_none() {
                    break Outcome::Looped { pc: pc_before };
                }
            }
        }
    };
    Run {
        uart: String::from_utf8_lossy(&bus.uart).replace('\r', ""),
        steps,
        outcome,
        touched: bus.touched.iter().copied().collect(),
        unmapped: bus.unmapped.clone(),
        aborts,
        sctlr: cpu.cp15(0, 1, 0, 0).unwrap_or(0),
        events,
    }
}

/// Little-endian bytes of A32 words.
pub fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Reads the first `n` bytes of a file (for sanity checks on fixtures).
pub fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("cannot read fixture {name}: {e}"))
}

/// A bus of plain RAM for instruction-level tests.
pub fn ram_bus(base: u64, len: usize) -> RamBus {
    RamBus::new(base, len)
}
