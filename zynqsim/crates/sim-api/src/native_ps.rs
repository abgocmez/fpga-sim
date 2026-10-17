//! The native PS engine (`backends.ps = "native"`, milestone P5, ADR-033).
//!
//! The PS is the pure-Rust Cortex-A9 interpreter `cpu-a9` on the address map and the
//! peripheral models of `mem-sys` and `ps-periph`. Core 0 runs the project's `[cpu0] elf`;
//! core 1 stays parked (it is never stepped, as the boot ROM leaves it). There is no PL: the
//! native PS attaches no programmable logic, so `backends.pl` must be `demo`.
//!
//! **Fidelity F0.** One instruction is one CPU cycle at 666.67 MHz, so one cycle is 1500 ps
//! exactly. Peripherals tick every [`BATCH_CYCLES`] cycles, and the core samples its interrupt
//! line before every instruction, so an interrupt is taken at most 48 ns after its device
//! raised it. This is not cycle-accurate: no caches, no bus contention, no DDR timing.
//!
//! Determinism: the engine reads no clock and no host input, so a session gives the same trace
//! digest in every process. Snapshots are re-execution from the journal (as on the QEMU
//! engine), and a restore must reproduce the snapshot's trace digest and state hash.

use crate::bytelog::ByteLog;
use crate::elf::Symbols;
use crate::engine::Engine;
use crate::error::ApiError;
use crate::qemu_engine::{cpsr_mode, resolve_locations, source_ref};
use crate::recording::{Recording, SimConfig};
use crate::source::SourceMap;
use crate::types::{
    BreakKind, BreakpointInfo, Capability, Command, CpuRegs, DebugStatus, DisasmLine, Fidelity,
    Input, Query, Response, SourceLine, Status, SymbolInfo, TraceFormat,
};
use board_zybo::{BoardDef, BoardState};
use cpu_a9::{Cpu, Mode, StepResult};
use mem_sys::{Bus, BusResult, OCM_BASE, OCM_SIZE, SystemBus, fnv1a64};
use object::{Object, ObjectSegment};
use ps_periph::system::{PsConfig, zynq_system};
use ps_periph::uart::Uart;
use sim_core::Ps;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use trace::{TraceEvent, TraceLog};

/// Picoseconds per CPU cycle: 666.67 MHz is a 1.5 ns period.
pub const PS_PER_CYCLE: u64 = 1_500;
/// Cycles between two device ticks (128 cycles = 192 ns). An interrupt raised by a device is
/// taken by the core at most this long after the device raised it, well inside the 1 us the
/// native PS promises for timer interrupts.
pub const BATCH_CYCLES: u64 = 128;
/// The interrupt latency bound: a device tick every `BATCH_CYCLES` cycles, which must stay under
/// the 1 us the native PS promises for timer interrupts (checked at compile time).
const _: () = assert!(BATCH_CYCLES * PS_PER_CYCLE < 1_000_000);
/// Trace records kept in memory; older ones are dropped (the digest keeps counting).
const TRACE_RETAIN: usize = 1_000_000;
/// Journal limits, as the QEMU engine has them: beyond them snapshots and recordings say so.
const JOURNAL_MAX_STEPS: usize = 2_000_000;
const JOURNAL_MAX_BYTES: usize = 64 << 20;
/// Largest memory access a query or a host write may make.
const MEM_MAX: usize = 1 << 20;
/// UART0 and UART1 register blocks, and the control register (`CR`) bits the bring-up sets.
const UART0_BASE: u64 = 0xE000_0000;
const UART1_BASE: u64 = 0xE000_1000;
const UART_CR: u64 = 0x00;
const UART_BAUDGEN: u64 = 0x18;
const UART_BAUDDIV: u64 = 0x34;
const CR_RX_EN: u32 = 1 << 2;
const CR_TX_EN: u32 = 1 << 4;

/// What the native engine needs to start.
#[derive(Clone, Debug)]
pub struct NativeSpec {
    /// Core 0 software: a 32-bit little-endian ARM ELF whose loadable segments lie in DDR or OCM.
    pub elf: PathBuf,
    /// The board description (it names the board; the native PS drives none of its pins yet).
    pub board_def: BoardDef,
    /// The quantum `Step` advances by, in nanoseconds.
    pub quantum_ns: u64,
    /// Execution breakpoints planted at start: addresses, ELF symbols or `file.rs:LINE`.
    pub breakpoints: Vec<String>,
    /// Directories searched for the sources of the ELF's line table, besides its compilation
    /// directory.
    pub source_dirs: Vec<PathBuf>,
}

/// The loadable part of an ELF: the entry point and the segments (addresses and bytes, with
/// `.bss` zero-filled).
struct Image {
    entry: u32,
    segments: Vec<(u64, Vec<u8>)>,
}

fn load_image(path: &Path) -> Result<Image, ApiError> {
    let bytes = std::fs::read(path)
        .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
    let bad = |why: String| {
        ApiError::invalid_argument(format!(
            "{} cannot run on the native PS: {why}",
            path.display()
        ))
    };
    let file = object::File::parse(&*bytes).map_err(|e| bad(e.to_string()))?;
    if file.is_64() || !file.is_little_endian() || file.architecture() != object::Architecture::Arm
    {
        return Err(bad("expected a 32-bit little-endian ARM ELF".to_string()));
    }
    let entry = u32::try_from(file.entry())
        .map_err(|_| bad("the entry point is above 4 GiB".to_string()))?;
    let mut segments = Vec::new();
    for seg in file.segments() {
        let data = seg.data().map_err(|e| bad(e.to_string()))?;
        let mut owned = data.to_vec();
        owned.resize(seg.size() as usize, 0);
        if !owned.is_empty() {
            segments.push((seg.address(), owned));
        }
    }
    Ok(Image { entry, segments })
}

/// One register write of the bring-up.
fn uart_write(bus: &mut SystemBus, addr: u64, value: u32) -> Result<(), ApiError> {
    bus.write(addr, 4, u64::from(value))
        .map_err(|e| ApiError::internal(format!("UART bring-up at {addr:#x}: {e:?}")))
}

/// The stop at a breakpoint, as the QEMU engine reports it.
struct StopNote {
    core: u8,
    pc: u32,
    reason: String,
}

/// A named point in the session, verified on restore.
struct NativeSnapshot {
    name: String,
    steps: usize,
    cycle: u64,
    target_ps: u64,
    trace_len: usize,
    trace_hash: String,
    state_hash: u64,
}

/// The native PS engine: core 0 on the PS bus, no PL. See the module documentation.
pub struct NativePsEngine {
    spec: NativeSpec,
    def: BoardDef,
    board: BoardState,
    bus: SystemBus,
    uart0: Arc<Mutex<Uart>>,
    uart1: Arc<Mutex<Uart>>,
    cpu: Cpu,
    /// CPU cycles executed so far.
    cycle: u64,
    /// The virtual time requested so far; the cycle count never passes it.
    target_ps: u64,
    quantum_ps: u64,
    trace: TraceLog,
    uart0_log: ByteLog,
    uart1_log: ByteLog,
    symbols: Symbols,
    source: SourceMap,
    breakpoints: Vec<BreakpointInfo>,
    stop: Option<StopNote>,
    stops: u32,
    /// The first instruction after a resume is not checked against breakpoints (the stop
    /// happened before it).
    resume_past_break: bool,
    /// An unimplemented instruction stopped the run; every later run reports it again.
    fault: Option<String>,
    /// Every state-changing request applied so far, run targets normalised to `RunUntil`.
    journal: Vec<Command>,
    journal_bytes: usize,
    journal_ok: bool,
    snapshots: Vec<NativeSnapshot>,
}

impl NativePsEngine {
    /// Builds the PS, loads the firmware and resets core 0 to its entry point.
    pub fn new(spec: &NativeSpec) -> Result<NativePsEngine, ApiError> {
        let image = load_image(&spec.elf)?;
        let (mut bus, handles) = zynq_system(&PsConfig::default())
            .map_err(|e| ApiError::internal(format!("the PS address map: {e}")))?;
        for (addr, data) in &image.segments {
            bus.load(*addr, data).map_err(|_| {
                ApiError::invalid_argument(format!(
                    "{}: a segment at {addr:#010x} ({} bytes) is not in DDR or OCM",
                    spec.elf.display(),
                    data.len()
                ))
            })?;
        }
        // Power-up bring-up of both UARTs: 115200 8N1 (BAUDGEN 124, BDIV 6 from the 100 MHz
        // reference), both directions enabled. On hardware the FSBL does this; the Xilinx QEMU
        // model transmits from reset, and the firmware runtime (`firmware/rt`) prints without
        // configuring UART1 at all. The reset baud rate (9600) would make each character cost
        // about a millisecond of virtual time.
        for base in [UART0_BASE, UART1_BASE] {
            uart_write(&mut bus, base + UART_BAUDGEN, 124)?;
            uart_write(&mut bus, base + UART_BAUDDIV, 6)?;
            uart_write(&mut bus, base + UART_CR, CR_TX_EN | CR_RX_EN)?;
        }
        let mut cpu = Cpu::new(0);
        cpu.reset(image.entry);
        let symbols = Symbols::load(&spec.elf).unwrap_or_default();
        let source = SourceMap::load(&spec.elf).unwrap_or_default();
        let mut breakpoints = Vec::new();
        for b in &spec.breakpoints {
            for addr in resolve_locations(b, &symbols, &source)? {
                breakpoints.push(exec_breakpoint(addr, &symbols, &source));
            }
        }
        let mut trace = TraceLog::with_limit(TRACE_RETAIN);
        let elf = spec
            .elf
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        trace.push(
            Ps::ZERO,
            TraceEvent::Marker {
                text: format!(
                    "native start elf={elf} core1=parked quantum_ns={} cycle_ps={PS_PER_CYCLE}",
                    spec.quantum_ns
                ),
            },
        );
        Ok(NativePsEngine {
            def: spec.board_def.clone(),
            board: BoardState::new(&spec.board_def),
            spec: spec.clone(),
            bus,
            uart0: handles.uart0,
            uart1: handles.uart1,
            cpu,
            cycle: 0,
            target_ps: 0,
            quantum_ps: spec.quantum_ns.saturating_mul(1000),
            trace,
            uart0_log: ByteLog::default(),
            uart1_log: ByteLog::default(),
            symbols,
            source,
            breakpoints,
            stop: None,
            stops: 0,
            resume_past_break: false,
            fault: None,
            journal: Vec::new(),
            journal_bytes: 0,
            journal_ok: true,
            snapshots: Vec::new(),
        })
    }

    fn now_ps(&self) -> Ps {
        Ps(self.cycle * PS_PER_CYCLE)
    }

    /// Runs `n` cycles of core 0 with the peripherals ticked after them.
    ///
    /// The interrupt line is sampled before an instruction whenever the bus was touched since
    /// the last sample (a device changes state only on an access or a tick), so the core sees
    /// every change before the instruction that follows it. Compute-only instructions do not
    /// pay for the sample.
    fn run_batch(&mut self, n: u64) {
        let mut done = 0u64;
        let mut fault = None;
        let mut watch = Watch {
            bus: &mut self.bus,
            dirty: true,
        };
        while done < n {
            if watch.dirty {
                let irq = watch.bus.core_irq(0);
                self.cpu.irq(irq);
                watch.dirty = false;
            }
            let pc = self.cpu.pc();
            if !self.resume_past_break && self.breakpoints.iter().any(|b| b.addr == u64::from(pc)) {
                self.halt(pc);
                break;
            }
            self.resume_past_break = false;
            match self.cpu.step(&mut watch) {
                StepResult::Executed { .. } => done += 1,
                StepResult::Unimplemented { pc, insn } => {
                    fault = Some((pc, insn));
                    break;
                }
            }
        }
        if done > 0 {
            self.bus.tick(done);
            self.cycle += done;
        }
        if let Some((pc, insn)) = fault {
            self.fault_at(pc, insn);
        }
    }

    /// Moves the UART output into the logs and the trace, stamped with the current time.
    fn flush_uart(&mut self) {
        let now = self.now_ps();
        let tx0 = self
            .uart0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take_tx();
        let tx1 = self
            .uart1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take_tx();
        for (port, bytes, log) in [
            (0u8, tx0, &mut self.uart0_log),
            (1u8, tx1, &mut self.uart1_log),
        ] {
            for b in bytes {
                log.push(b);
                self.trace.push(
                    now,
                    TraceEvent::Uart {
                        port,
                        tx: true,
                        byte: b,
                    },
                );
            }
        }
    }

    /// Stops before the instruction at `pc` runs.
    fn halt(&mut self, pc: u32) {
        let reason = describe_pc(pc, &self.symbols, &self.source);
        self.stops += 1;
        let text = format!("core 0 stopped: breakpoint at {reason}");
        self.stop = Some(StopNote {
            core: 0,
            pc,
            reason: text.clone(),
        });
        let now = self.now_ps();
        self.trace.push(now, TraceEvent::Marker { text });
    }

    /// An instruction this core does not implement stops the engine: a message names the
    /// address, the word and the state, and nothing wrong is computed.
    fn fault_at(&mut self, pc: u32, insn: u32) {
        let thumb = self.cpu.cpsr() & (1 << 5) != 0;
        let kind = if thumb {
            "Thumb halfword"
        } else {
            "A32 instruction word"
        };
        let msg = format!(
            "native PS stopped at {}: unimplemented {kind} {insn:#010x} at cycle {} \
             (Thumb and VFP/NEON are not implemented; docs/adr/033-native-ps-engine.md). \
             The instruction was not executed and no state changed",
            describe_pc(pc, &self.symbols, &self.source),
            self.cycle
        );
        let now = self.now_ps();
        self.trace.push(
            now,
            TraceEvent::Marker {
                text: format!("native fault: {msg}"),
            },
        );
        self.fault = Some(msg);
    }

    fn advance_to_cycle(&mut self, end: u64) -> Result<(), ApiError> {
        if let Some(f) = &self.fault {
            return Err(ApiError::unsupported(f.clone()));
        }
        if self.stop.take().is_some() {
            self.resume_past_break = true;
        }
        while self.cycle < end && self.stop.is_none() && self.fault.is_none() {
            let n = (end - self.cycle).min(BATCH_CYCLES);
            self.run_batch(n);
            self.flush_uart();
        }
        match &self.fault {
            Some(f) => Err(ApiError::unsupported(f.clone())),
            None => Ok(()),
        }
    }

    /// Advances to the virtual time `time_ps` (never backwards).
    fn run_until_ps(&mut self, time_ps: u64) -> Result<(), ApiError> {
        self.target_ps = self.target_ps.max(time_ps);
        self.advance_to_cycle(self.target_ps / PS_PER_CYCLE)
    }

    /// The journal of state-changing requests, when it is still complete.
    fn journal_needed(&self, what: &str) -> Result<(), ApiError> {
        if self.journal_ok {
            Ok(())
        } else {
            Err(unsupported(
                what,
                "the session grew past the journal limits (2 million requests or 64 MiB of \
                 inputs); reset to start recording again",
            ))
        }
    }

    fn record(&mut self, c: Command) {
        if !self.journal_ok {
            return;
        }
        let payload = match &c {
            Command::MemWrite { data, .. } => data.len(),
            _ => 0,
        };
        let bytes = payload + 48;
        self.journal_bytes += bytes;
        if self.journal.len() >= JOURNAL_MAX_STEPS || self.journal_bytes > JOURNAL_MAX_BYTES {
            self.journal_ok = false;
            self.journal = Vec::new();
            self.snapshots.clear();
            return;
        }
        self.journal.push(c);
    }

    /// A digest of everything that determines the future of the session: the bus (RAM,
    /// devices, cycle count), core 0's registers and mode, the requested time and the stop.
    pub fn state_hash(&self) -> u64 {
        let mut b = Vec::with_capacity(256);
        b.extend_from_slice(&self.bus.state_hash().to_le_bytes());
        b.extend_from_slice(&self.cycle.to_le_bytes());
        b.extend_from_slice(&self.target_ps.to_le_bytes());
        for n in 0..16u8 {
            b.extend_from_slice(&self.cpu.reg(n).to_le_bytes());
        }
        b.extend_from_slice(&self.cpu.cpsr().to_le_bytes());
        for mode in [
            Mode::Fiq,
            Mode::Irq,
            Mode::Supervisor,
            Mode::Abort,
            Mode::Undefined,
        ] {
            b.extend_from_slice(&self.cpu.spsr(mode).to_le_bytes());
        }
        b.push(u8::from(self.cpu.is_waiting()));
        b.extend_from_slice(&self.stops.to_le_bytes());
        b.push(u8::from(self.resume_past_break));
        fnv1a64(&b)
    }

    /// Whether `addr..addr+len` lies in DDR or OCM, the only memory a host may read or write
    /// directly (device registers have side effects).
    fn in_ram(&self, addr: u64, len: u64) -> bool {
        let end = addr.checked_add(len);
        let ddr = end.is_some_and(|e| e <= self.bus.ddr_size());
        let ocm = addr >= OCM_BASE && end.is_some_and(|e| e <= OCM_BASE + OCM_SIZE);
        ddr || ocm
    }

    fn read_ram(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, ApiError> {
        if !self.in_ram(addr, len as u64) {
            return Err(ApiError::invalid_argument(format!(
                "{len} byte(s) at {addr:#x} are not in DDR or OCM: device registers are not \
                 read by a query"
            )));
        }
        (0..len as u64)
            .map(|i| {
                self.bus
                    .read(addr + i, 1)
                    .map(|v| v as u8)
                    .map_err(|e| ApiError::internal(format!("read at {:#x}: {e:?}", addr + i)))
            })
            .collect()
    }

    fn restore_by_reexecution(&mut self, name: &str) -> Result<Response, ApiError> {
        self.journal_needed("snapshot")?;
        let snap = self
            .snapshots
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| ApiError::not_found(format!("no snapshot named `{name}`")))?;
        let (steps, cycle, target_ps, trace_len, hash, state) = (
            snap.steps,
            snap.cycle,
            snap.target_ps,
            snap.trace_len,
            snap.trace_hash.clone(),
            snap.state_hash,
        );
        let mut fresh = self.rebuild(&self.journal[..steps])?;
        if fresh.trace.hash_string() != hash
            || fresh.trace.len() != trace_len
            || fresh.cycle != cycle
            || fresh.target_ps != target_ps
            || fresh.state_hash() != state
        {
            return Err(ApiError::internal(format!(
                "re-execution did not reproduce snapshot `{name}` (trace digest {} vs {hash}, \
                 state hash {:#018x} vs {state:#018x})",
                fresh.trace.hash_string(),
                fresh.state_hash()
            )));
        }
        fresh.snapshots = std::mem::take(&mut self.snapshots)
            .into_iter()
            .filter(|s| s.steps <= steps)
            .collect();
        std::mem::swap(self, &mut fresh);
        Ok(Response::Ok)
    }

    /// A fresh engine from the same spec with `steps` applied.
    fn rebuild(&self, steps: &[Command]) -> Result<NativePsEngine, ApiError> {
        let mut fresh = NativePsEngine::new(&self.spec)?;
        for c in steps {
            fresh.command(c.clone())?;
        }
        Ok(fresh)
    }

    fn apply(&mut self, cmd: Command) -> Result<Response, ApiError> {
        match cmd {
            Command::RunUntil { time_ps } => {
                self.run_until_ps(time_ps)?;
                Ok(Response::Time {
                    now_ps: self.now_ps().0,
                })
            }
            Command::Input(Input::UartWrite { port, data }) => {
                let uart = match port {
                    0 => &self.uart0,
                    1 => &self.uart1,
                    p => {
                        return Err(ApiError::invalid_argument(format!(
                            "no uart {p}: the native PS has UART0 and UART1"
                        )));
                    }
                };
                let n = uart
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .inject_rx(&data);
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("uart{port} write {n} byte(s)"),
                    },
                );
                if n < data.len() {
                    return Err(ApiError::invalid_argument(format!(
                        "uart{port}: only {n} of {} byte(s) fit the receive FIFO",
                        data.len()
                    )));
                }
                Ok(Response::Ok)
            }
            Command::Input(Input::SetSwitch { .. } | Input::SetButton { .. }) => Err(unsupported(
                "board inputs",
                "the native PS does not drive the board pins yet (no PL is attached; PL \
                 attachment is out of scope for this engine, use `backends.ps = \"qemu\"`)",
            )),
            Command::Input(Input::Reset { .. }) => self.reset_in_place(),
            Command::AttachDevice { .. } | Command::SetDeviceValue { .. } => Err(unsupported(
                "pmod devices",
                "Pmod devices are attached through the PL, which the native PS does not have",
            )),
            Command::SetMediaSource { .. } => Err(unsupported(
                "media sources",
                "HDMI and the codec are PL models; the native PS has no PL",
            )),
            Command::SetQuantum { .. } => Err(unsupported(
                "set_quantum",
                "the quantum is fixed when the engine starts (`[sim] quantum_ns`)",
            )),
            Command::Play { .. } | Command::Pause => Err(ApiError::unsupported(
                "continuous running needs the simulation server (SimClient)",
            )),
            Command::SetBreakpoint {
                addr, kind, enable, ..
            } => {
                if kind != BreakKind::Exec {
                    return Err(unsupported(
                        "watchpoint",
                        "only execution breakpoints are implemented on the native PS",
                    ));
                }
                let a = u32::try_from(addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                self.breakpoints.retain(|b| b.addr != addr);
                if enable {
                    self.breakpoints
                        .push(exec_breakpoint(a, &self.symbols, &self.source));
                }
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!(
                            "{} Exec breakpoint at {addr:#x}",
                            if enable { "set" } else { "clear" }
                        ),
                    },
                );
                Ok(Response::Ok)
            }
            Command::MemWrite { addr, data } => {
                if data.is_empty() || data.len() > MEM_MAX {
                    return Err(ApiError::invalid_argument(
                        "data must be between 1 byte and 1 MiB",
                    ));
                }
                if !self.in_ram(addr, data.len() as u64) {
                    return Err(ApiError::invalid_argument(format!(
                        "{} byte(s) at {addr:#x} are not in DDR or OCM",
                        data.len()
                    )));
                }
                self.bus
                    .load(addr, &data)
                    .map_err(|_| ApiError::invalid_argument("memory write refused"))?;
                let now = self.now_ps();
                self.trace.push(
                    now,
                    TraceEvent::Input {
                        what: format!("mem write {addr:#x} len {}", data.len()),
                    },
                );
                Ok(Response::Ok)
            }
            Command::Run { .. }
            | Command::Step
            | Command::SnapshotSave { .. }
            | Command::SnapshotRestore { .. } => {
                unreachable!("normalised or handled before apply")
            }
        }
    }

    /// Starts over with the same firmware, as the QEMU engine's reset does.
    fn reset_in_place(&mut self) -> Result<Response, ApiError> {
        let mut fresh = NativePsEngine::new(&self.spec)?;
        std::mem::swap(self, &mut fresh);
        Ok(Response::Ok)
    }

    fn snapshot_save(&mut self, name: String) -> Result<Response, ApiError> {
        self.journal_needed("snapshot")?;
        self.snapshots.retain(|s| s.name != name);
        self.snapshots.push(NativeSnapshot {
            name,
            steps: self.journal.len(),
            cycle: self.cycle,
            target_ps: self.target_ps,
            trace_len: self.trace.len(),
            trace_hash: self.trace.hash_string(),
            state_hash: self.state_hash(),
        });
        Ok(Response::Ok)
    }
}

/// The core's view of the PS bus. Every access is forwarded to core 0. An access outside DDR
/// and OCM may change device state (a read can too: an interrupt acknowledge does), so it marks
/// the bus as changed; RAM accesses (instruction fetches included) cannot change a device.
struct Watch<'a> {
    bus: &'a mut SystemBus,
    dirty: bool,
}

impl Watch<'_> {
    fn touches_devices(&self, addr: u64) -> bool {
        !(addr < self.bus.ddr_size() || (OCM_BASE..OCM_BASE + OCM_SIZE).contains(&addr))
    }
}

impl Bus for Watch<'_> {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        if self.touches_devices(addr) {
            self.dirty = true;
        }
        Bus::read(self.bus, addr, size)
    }

    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        if self.touches_devices(addr) {
            self.dirty = true;
        }
        Bus::write(self.bus, addr, size, value)
    }
}

/// `pc=0x... (symbol+off, file:line)`, the parts that are known.
fn describe_pc(pc: u32, symbols: &Symbols, source: &SourceMap) -> String {
    let sym = symbols.containing(pc).map(|(s, off)| {
        if off == 0 {
            s.name.clone()
        } else {
            format!("{}+{off:#x}", s.name)
        }
    });
    let at = source_ref(source, pc).map(|s| format!("{}:{}", s.file, s.line));
    let tail = match (sym, at) {
        (None, None) => String::new(),
        (Some(s), None) => format!(" ({s})"),
        (None, Some(a)) => format!(" ({a})"),
        (Some(s), Some(a)) => format!(" ({s}, {a})"),
    };
    format!("pc={pc:#010x}{tail}")
}

fn exec_breakpoint(addr: u32, symbols: &Symbols, source: &SourceMap) -> BreakpointInfo {
    BreakpointInfo {
        addr: u64::from(addr),
        kind: BreakKind::Exec,
        len: 4,
        symbol: symbols.containing(addr).map(|(s, _)| s.name.clone()),
        source: source_ref(source, addr),
    }
}

fn unsupported(what: &str, why: &str) -> ApiError {
    ApiError::unsupported(format!(
        "`{what}` is not available on the `native` backend: {why}"
    ))
}

impl Engine for NativePsEngine {
    fn def(&self) -> &BoardDef {
        &self.def
    }

    fn board(&self) -> &BoardState {
        &self.board
    }

    fn trace(&self) -> &TraceLog {
        &self.trace
    }

    fn status(&self, playing: bool) -> Status {
        let stop = self.stop.as_ref();
        Status {
            now_ps: self.now_ps().0,
            playing,
            quantum_ps: self.quantum_ps,
            seed: 0,
            board: self.def.name.clone(),
            ps_backend: "native".to_string(),
            pl_backend: "none".to_string(),
            fidelity: Fidelity::F0,
            capabilities: vec![
                Capability::Uart,
                Capability::Trace,
                Capability::MemoryAccess,
                Capability::CpuState,
                Capability::Snapshot,
                Capability::Replay,
            ],
            trace_len: self.trace.len(),
            trace_hash: self.trace.hash_string(),
            pending_events: 0,
            devices: Vec::new(),
            debug: Some(DebugStatus {
                stopped: stop.is_some(),
                core: stop.map(|s| s.core),
                pc: stop.map(|s| s.pc),
                reason: stop.map(|s| s.reason.clone()),
                stops: self.stops,
            }),
            media: None,
            storage: None,
        }
    }

    fn command(&mut self, cmd: Command) -> Result<Response, ApiError> {
        match cmd {
            Command::SnapshotSave { name } => return self.snapshot_save(name),
            Command::SnapshotRestore { name } => return self.restore_by_reexecution(&name),
            Command::Input(Input::Reset { .. }) => return self.reset_in_place(),
            _ => {}
        }
        // Run targets are journaled as absolute times, so a replay slices the run as it
        // was sliced here.
        let now = self.now_ps().0;
        let cmd = match cmd {
            Command::Run { duration_ps } => Command::RunUntil {
                time_ps: self.target_ps.max(now) + duration_ps,
            },
            Command::Step => Command::RunUntil {
                time_ps: self.target_ps.max(now) + self.quantum_ps,
            },
            other => other,
        };
        let journaled = !matches!(cmd, Command::Play { .. } | Command::Pause);
        let r = self.apply(cmd.clone());
        if r.is_ok() && journaled {
            self.record(cmd);
        }
        r
    }

    fn query(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        self.query_inner(q, playing)
    }

    fn run_for(&mut self, d: Ps) -> Result<(), ApiError> {
        self.command(Command::Run { duration_ps: d.0 }).map(|_| ())
    }

    fn halted(&self) -> Option<String> {
        self.stop.as_ref().map(|s| s.reason.clone())
    }
}

impl NativePsEngine {
    fn query_inner(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        match q {
            Query::Status => Ok(Response::Status(Box::new(self.status(playing)))),
            Query::BoardDef => Ok(Response::BoardDef(Box::new(self.def.clone()))),
            Query::BoardState => Ok(Response::BoardState(self.board.clone())),
            Query::Pin { name } => self
                .board
                .pin_level(&self.def, name)
                .map(|level| Response::Pin {
                    name: name.clone(),
                    level,
                })
                .ok_or_else(|| ApiError::not_found(format!("no pin or control named `{name}`"))),
            Query::UartRead { port, from, max } => {
                let log = match port {
                    0 => &self.uart0_log,
                    1 => &self.uart1_log,
                    p => return Err(ApiError::invalid_argument(format!("no uart {p}"))),
                };
                let (data, next) = log.read(*from, *max);
                Ok(Response::Uart {
                    data,
                    next,
                    oldest: log.base,
                })
            }
            Query::Trace { from, max } => {
                let (start, recs) = self.trace.window(*from, *max);
                Ok(Response::Trace {
                    records: recs.to_vec(),
                    next: start + recs.len(),
                    total: self.trace.len(),
                    hash: self.trace.hash_string(),
                })
            }
            Query::TraceExport { format } => Ok(Response::Text(match format {
                TraceFormat::Vcd => trace::to_vcd(&self.trace),
                TraceFormat::Perfetto => trace::to_perfetto_json(&self.trace),
            })),
            Query::SignalCatalogue | Query::Waveform { .. } => crate::wave::answer(&self.trace, q),
            Query::Snapshots => Ok(Response::Snapshots(
                self.snapshots.iter().map(|s| s.name.clone()).collect(),
            )),
            Query::Recording => {
                self.journal_needed("recording")?;
                Ok(Response::Recording(Box::new(Recording {
                    version: 1,
                    config: SimConfig {
                        board: self.def.name.clone(),
                        board_def: (BoardDef::builtin(&self.def.name).as_ref() != Some(&self.def))
                            .then(|| Box::new(self.def.clone())),
                        seed: 0,
                        quantum_ps: self.quantum_ps,
                    },
                    entries: Vec::new(),
                    end_ps: self.now_ps().0,
                    trace_hash: self.trace.hash_string(),
                    journal: self.journal.clone(),
                    project: None,
                })))
            }
            Query::Breakpoints => Ok(Response::Breakpoints(self.breakpoints.clone())),
            Query::CpuRegs { core } => {
                if *core != 0 {
                    return Err(unsupported(
                        "cpu_regs",
                        "core 1 is parked and not modelled on the native PS",
                    ));
                }
                let r: [u32; 16] = std::array::from_fn(|n| self.cpu.reg(n as u8));
                let cpsr = self.cpu.cpsr();
                let pc_symbol = self.symbols.containing(r[15]).map(|(s, off)| {
                    if off == 0 {
                        s.name.clone()
                    } else {
                        format!("{}+{off:#x}", s.name)
                    }
                });
                Ok(Response::CpuRegs(CpuRegs {
                    core: 0,
                    r,
                    cpsr,
                    mode: cpsr_mode(cpsr).to_string(),
                    pc_symbol,
                    pc_source: source_ref(&self.source, r[15]),
                }))
            }
            Query::MemRead { addr, len } => {
                if *len == 0 || *len as usize > MEM_MAX {
                    return Err(ApiError::invalid_argument(
                        "len must be between 1 byte and 1 MiB",
                    ));
                }
                let data = self.read_ram(*addr, *len as usize)?;
                Ok(Response::Memory { addr: *addr, data })
            }
            Query::Disasm { addr, count, thumb } => {
                if *count == 0 || *count > 256 {
                    return Err(ApiError::invalid_argument(
                        "count must be between 1 and 256",
                    ));
                }
                let a = u32::try_from(*addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                let thumb = thumb.unwrap_or(false);
                let bytes = self.read_ram(u64::from(a), (*count as usize) * 4)?;
                let lines = crate::disasm::disassemble(&bytes, a, thumb, *count as usize)
                    .into_iter()
                    .map(|l| DisasmLine {
                        addr: u64::from(l.addr),
                        size: l.size,
                        hex: l.hex,
                        text: l.text,
                        symbol: self.symbols.containing(l.addr).map(|(s, off)| {
                            if off == 0 {
                                s.name.clone()
                            } else {
                                format!("{}+{off:#x}", s.name)
                            }
                        }),
                    })
                    .collect();
                Ok(Response::Disasm { thumb, lines })
            }
            Query::Symbol { query } => {
                let symbols = &self.symbols;
                if symbols.is_empty() {
                    return Err(ApiError::not_found("the ELF has no symbols"));
                }
                let t = query.trim();
                let addr = t
                    .strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok());
                let found = match addr {
                    Some(a) => symbols.containing(a).map(|(s, off)| (s, u64::from(off))),
                    None => symbols.by_name(t).map(|s| (s, 0)),
                };
                let (s, offset) =
                    found.ok_or_else(|| ApiError::not_found(format!("no symbol for `{t}`")))?;
                Ok(Response::Symbol(SymbolInfo {
                    name: s.name.clone(),
                    addr: u64::from(s.addr),
                    size: u64::from(s.size),
                    offset,
                }))
            }
            Query::SourceLine { addr } => {
                let a = u32::try_from(*addr).map_err(|_| {
                    ApiError::invalid_argument("addresses above 4 GiB do not exist on this core")
                })?;
                if self.source.is_empty() {
                    return Err(ApiError::not_found(
                        "the ELF has no line information (build the firmware with debug info, \
                         for example `debug = \"line-tables-only\"`)",
                    ));
                }
                let l = self.source.lookup(a).ok_or_else(|| {
                    ApiError::not_found(format!("no source line for address {a:#x}"))
                })?;
                Ok(Response::SourceLine(SourceLine {
                    addr: *addr,
                    file: l.file.to_string(),
                    line: l.line,
                    column: l.column,
                    function: l
                        .function
                        .map(str::to_string)
                        .or_else(|| self.symbols.containing(a).map(|(s, _)| s.name.clone())),
                }))
            }
            Query::SourceFile { path } => {
                let (resolved, text) = self
                    .source
                    .read_source(path, &self.spec.source_dirs)
                    .map_err(ApiError::not_found)?;
                Ok(Response::SourceFile {
                    file: path.clone(),
                    resolved: resolved.to_string_lossy().into_owned(),
                    text,
                })
            }
            Query::SourceAddrs { file, line } => {
                let f = self
                    .source
                    .addrs_for(file, *line)
                    .map_err(ApiError::not_found)?;
                Ok(Response::SourceAddrs {
                    file: f.file,
                    line: f.line,
                    addrs: f.addrs.into_iter().map(u64::from).collect(),
                })
            }
            Query::PlSignal { .. } => Err(unsupported("pl_signal_get", "the native PS has no PL")),
            Query::Screenshot { .. }
            | Query::VideoFrame
            | Query::AudioCapture { .. }
            | Query::AudioFrom { .. }
            | Query::DeviceState { .. }
            | Query::DeviceImage { .. }
            | Query::NetCapture { .. }
            | Query::NetStats
            | Query::NetPcap => Err(unsupported(
                "this query",
                "the native PS drives no board pictures, sound, Pmod devices or Ethernet yet",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Project, Severity};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/elf")
            .join(name)
    }

    fn spec_for(elf: PathBuf) -> NativeSpec {
        NativeSpec {
            elf,
            board_def: BoardDef::builtin("zybo-z7-20").expect("built-in board"),
            quantum_ns: 1000,
            breakpoints: Vec::new(),
            source_dirs: Vec::new(),
        }
    }

    fn hello() -> NativeSpec {
        spec_for(fixture("rust_hello.elf"))
    }

    fn uart_text(e: &mut NativePsEngine) -> String {
        match e.query(
            &Query::UartRead {
                port: 1,
                from: 0,
                max: 1 << 16,
            },
            false,
        ) {
            Ok(Response::Uart { data, .. }) => String::from_utf8_lossy(&data).into_owned(),
            other => panic!("uart_read: {other:?}"),
        }
    }

    /// A one-segment ELF32 ARM executable: `code` loaded at `load` with entry `entry`.
    fn tiny_elf(entry: u32, load: u32, code: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; 84];
        e[..4].copy_from_slice(b"\x7fELF");
        e[4] = 1; // 32-bit
        e[5] = 1; // little endian
        e[6] = 1; // version
        e[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        e[18..20].copy_from_slice(&40u16.to_le_bytes()); // EM_ARM
        e[20..24].copy_from_slice(&1u32.to_le_bytes());
        e[24..28].copy_from_slice(&entry.to_le_bytes());
        e[28..32].copy_from_slice(&52u32.to_le_bytes()); // e_phoff
        e[40..42].copy_from_slice(&52u16.to_le_bytes()); // e_ehsize
        e[42..44].copy_from_slice(&32u16.to_le_bytes()); // e_phentsize
        e[44..46].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let ph = 52;
        let size = code.len() as u32;
        e[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        e[ph + 4..ph + 8].copy_from_slice(&84u32.to_le_bytes()); // p_offset
        e[ph + 8..ph + 12].copy_from_slice(&load.to_le_bytes()); // p_vaddr
        e[ph + 12..ph + 16].copy_from_slice(&load.to_le_bytes()); // p_paddr
        e[ph + 16..ph + 20].copy_from_slice(&size.to_le_bytes()); // p_filesz
        e[ph + 20..ph + 24].copy_from_slice(&size.to_le_bytes()); // p_memsz
        e[ph + 24..ph + 28].copy_from_slice(&7u32.to_le_bytes()); // p_flags
        e[ph + 28..ph + 32].copy_from_slice(&4u32.to_le_bytes()); // p_align
        e.extend_from_slice(code);
        e
    }

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zynqsim-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("temp elf");
        p
    }

    #[test]
    fn the_engine_is_send_because_the_server_thread_owns_it() {
        fn assert_send<T: Send>() {}
        assert_send::<NativePsEngine>();
    }

    #[test]
    fn rust_hello_prints_its_text_and_parks() {
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        e.run_for(Ps::from_ms(8)).expect("run");
        assert_eq!(
            uart_text(&mut e),
            "hello from Rust on the simulated Cortex-A9\r\nsum(1..=100) = 5050\r\nRESULT: ALL PASS\r\n"
        );
        let st = e.status(false);
        assert_eq!(st.ps_backend, "native");
        assert_eq!(st.pl_backend, "none");
        assert_eq!(st.fidelity, Fidelity::F0);
    }

    #[test]
    fn five_runs_give_one_trace_digest() {
        let digests: Vec<String> = (0..5)
            .map(|_| {
                let mut e = NativePsEngine::new(&hello()).expect("engine");
                e.run_for(Ps::from_ms(2)).expect("run");
                e.status(false).trace_hash
            })
            .collect();
        assert!(digests.iter().all(|d| *d == digests[0]), "{digests:?}");
    }

    #[test]
    fn a_breakpoint_stops_before_the_instruction_and_resumes() {
        let mut spec = hello();
        spec.breakpoints = vec!["main".to_string()];
        let mut e = NativePsEngine::new(&spec).expect("engine");
        let main = e.symbols.by_name("main").expect("main").addr;
        e.run_for(Ps::from_ms(8)).expect("run");
        let reason = e.halted().expect("stopped at main");
        assert!(reason.contains(&format!("{main:#010x}")), "{reason}");
        let st = e.status(false);
        let dbg = st.debug.expect("debug status");
        assert!(dbg.stopped && dbg.pc == Some(main) && dbg.stops == 1);
        assert_eq!(uart_text(&mut e), "", "nothing printed before main");
        // Resuming runs the program to its end; the breakpoint is not hit again.
        e.run_for(Ps::from_ms(8)).expect("resume");
        assert!(e.halted().is_none());
        assert!(uart_text(&mut e).ends_with("RESULT: ALL PASS\r\n"));
    }

    #[test]
    fn a_snapshot_restores_the_same_session_and_is_verified() {
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        e.run_for(Ps::from_ms(1)).expect("run");
        e.command(Command::SnapshotSave {
            name: "a".to_string(),
        })
        .expect("save");
        let at_snapshot = e.status(false).now_ps;
        e.run_for(Ps::from_ms(3)).expect("run on");
        let straight = e.status(false);
        e.command(Command::SnapshotRestore {
            name: "a".to_string(),
        })
        .expect("restore");
        assert_eq!(e.status(false).now_ps, at_snapshot);
        e.run_for(Ps::from_ms(3)).expect("run again");
        let again = e.status(false);
        assert_eq!(again.trace_hash, straight.trace_hash);
        assert_eq!(again.now_ps, straight.now_ps);
        let snaps = match e.query(&Query::Snapshots, false) {
            Ok(Response::Snapshots(s)) => s,
            other => panic!("{other:?}"),
        };
        assert_eq!(snaps, vec!["a".to_string()]);
    }

    #[test]
    fn an_unimplemented_instruction_stops_with_its_pc_and_word() {
        // mov r0, #1, then a VFP instruction (coprocessor 10), which is not implemented.
        let mut code = Vec::new();
        code.extend_from_slice(&0xE3A0_0001u32.to_le_bytes());
        code.extend_from_slice(&0xEEB0_0A40u32.to_le_bytes());
        let path = write_temp("vfp.elf", &tiny_elf(0x0010_0000, 0x0010_0000, &code));
        let mut e = NativePsEngine::new(&spec_for(path)).expect("engine");
        let err = e
            .run_for(Ps::from_us(10))
            .expect_err("VFP must stop the run");
        let msg = err.message.clone();
        assert!(msg.contains("pc=0x00100004"), "{msg}");
        assert!(msg.contains("0xeeb00a40"), "{msg}");
        assert_eq!(e.cpu.reg(0), 1, "the instruction before it ran");
        assert_eq!(
            e.cpu.pc(),
            0x0010_0004,
            "the faulting instruction did not run"
        );
        // The fault is sticky: a later run reports it again.
        assert!(e.run_for(Ps::from_us(1)).is_err());
    }

    #[test]
    fn memory_reads_see_the_loaded_image_and_device_space_is_refused() {
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        let image = std::fs::read(fixture("rust_hello.elf")).expect("fixture");
        let word = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().expect("4 bytes"));
        let entry = word(24);
        // The first PT_LOAD segment: its file offset (p_offset) holds the bytes at its address.
        let phoff = word(28) as usize;
        let file_off = word(phoff + 4) as usize;
        let seg_addr = word(phoff + 8);
        assert_eq!(seg_addr, entry, "rust_hello starts at its first segment");
        match e.query(
            &Query::MemRead {
                addr: u64::from(entry),
                len: 8,
            },
            false,
        ) {
            Ok(Response::Memory { data, .. }) => {
                assert_eq!(data.len(), 8);
                assert_eq!(data[..], image[file_off..file_off + 8]);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            e.query(
                &Query::MemRead {
                    addr: 0xE000_1030,
                    len: 4
                },
                false
            )
            .is_err(),
            "device registers are not read by a query"
        );
    }

    #[test]
    fn core_zero_starts_in_supervisor_mode_at_the_entry_point() {
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        match e.query(&Query::CpuRegs { core: 0 }, false) {
            Ok(Response::CpuRegs(r)) => {
                assert_eq!(r.mode, "svc");
                assert_eq!(r.cpsr & 0xC0, 0xC0, "IRQ and FIQ masked");
                assert_eq!(r.r[15], 0x0010_0000, "the entry of rust_hello");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            e.query(&Query::CpuRegs { core: 1 }, false).is_err(),
            "core 1 is parked and not modelled"
        );
    }

    #[test]
    fn uart_input_is_accepted_and_bad_ports_are_refused() {
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        assert!(matches!(
            e.command(Command::Input(Input::UartWrite {
                port: 1,
                data: b"abc".to_vec()
            })),
            Ok(Response::Ok)
        ));
        assert!(
            e.command(Command::Input(Input::UartWrite {
                port: 7,
                data: b"x".to_vec()
            }))
            .is_err()
        );
    }

    #[test]
    fn reset_returns_to_the_start_of_the_session() {
        let fresh = NativePsEngine::new(&hello()).expect("engine").status(false);
        let mut e = NativePsEngine::new(&hello()).expect("engine");
        e.run_for(Ps::from_ms(2)).expect("run");
        e.command(Command::Input(Input::Reset { seed: None }))
            .expect("reset");
        let st = e.status(false);
        assert_eq!(st.now_ps, 0);
        assert_eq!(st.trace_hash, fresh.trace_hash);
    }

    fn native_hello_text() -> String {
        std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/native-hello/project.toml"),
        )
        .expect("example project")
    }

    fn errors_of(text: &str) -> Vec<String> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/native-hello");
        let p = Project::from_toml(text).expect("parses");
        p.validate(&base)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.field, d.message))
            .collect()
    }

    #[test]
    fn validation_refuses_what_the_native_ps_cannot_attach() {
        let text = native_hello_text();
        assert!(errors_of(&text).is_empty(), "{:?}", errors_of(&text));
        let pl = text.replace("pl = \"demo\"", "pl = \"native\"");
        assert!(errors_of(&pl).iter().any(|e| e.starts_with("backends.pl:")));
        let devices = text.replace("[sim]", "[devices]\npmod_ja = \"oled\"\n\n[sim]");
        assert!(
            errors_of(&devices)
                .iter()
                .any(|e| e.starts_with("devices:"))
        );
        let media = text.replace("[sim]", "[media]\nvideo_in = \"bars\"\n\n[sim]");
        assert!(errors_of(&media).iter().any(|e| e.starts_with("media:")));
    }

    #[test]
    fn the_native_example_builds_a_native_engine() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/native-hello/project.toml");
        let p = Project::load(&path).expect("project");
        let e = p
            .build_engine(path.parent().expect("dir"))
            .expect("engine builds");
        let st = e.status(false);
        assert_eq!(st.ps_backend, "native");
        assert_eq!(st.fidelity, Fidelity::F0);
        assert_eq!(e.def().name, "zybo-z7-20");
    }
}
