//! The event-driven simulator.
//!
//! Scheduling follows the Verilog model. At one time step the **active** region runs
//! every ready process until it is empty; then the **non-blocking** queue is applied
//! in order, which may make more processes ready. This repeats until both are empty
//! (a delta cycle is one such round). Timed events (`#n`) move time forward to the
//! next one. A step with too many process activations or too many non-blocking
//! rounds is reported as a combinational loop.
//!
//! Values are 4-state (0, 1, X, Z), at most 128 bits wide, see [`crate::value`].
//! Storage follows the declaration (`docs/RTL_SUBSET.md`): a net (`wire`) is Z until
//! driven and takes the resolution of its continuous drivers; a `reg` or `logic` is X
//! until assigned; a `bit` or `int` is 0 and never holds X or Z.

use crate::design::Design;
use crate::ir::{Arg, Edge, Ex, Func, K, LVal, Op, ProcKind, SigId, SigKind, Sys, Trigger};
use crate::value::{
    Bit4, Value4, and, arith, ashr, case_eq, case_match, extend, land, lnot, logical_eq, lor, mask,
    merge, not, or, radix_bits, reduce_and, reduce_or, reduce_xor, resolve, shl, shr, xor,
};
use rtl_frontend::ast::{BinOp, UnOp};
use rtl_frontend::{Diagnostic, Pos};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

/// Operations one process activation (or one function call) may execute.
const STEP_BUDGET: u64 = 20_000_000;
/// Process activations in one time step before a combinational loop is reported.
const ACTIVATION_LIMIT: u64 = 2_000_000;
/// Non-blocking rounds in one time step before a combinational loop is reported.
const DELTA_LIMIT: u64 = 200_000;

/// A run-time failure of the simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimError {
    /// Simulation time when it happened, in picoseconds.
    pub time_ps: u64,
    /// What happened.
    pub message: String,
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at {} ps: {}", self.time_ps, self.message)
    }
}

impl std::error::Error for SimError {}

/// A value change of a scalar or vector signal, for waveform export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// Time in picoseconds.
    pub time_ps: u64,
    /// Signal.
    pub sig: SigId,
    /// New value.
    pub value: Value4,
}

/// A process registered on a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiter {
    proc: u32,
    epoch: u32,
    bit: Option<u32>,
    edge: Edge,
}

/// A pending non-blocking update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Nba {
    idx: usize,
    sig: SigId,
    lo: u32,
    w: u32,
    value: Value4,
}

/// One continuous driver of a net: what one `assign` (or port connection) drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Driver {
    /// The net.
    sig: SigId,
    /// The value it drives, in the net's bit positions (zero outside `cover`).
    val: Value4,
    /// The bits it drives.
    cover: u128,
}

/// How a run of bytecode ended.
enum Exit {
    /// Suspended on a wait (the next instruction to run).
    Wait(usize),
    /// Suspended on a delay (the next instruction to run).
    Delay(usize),
    /// Reached `Halt`.
    Halt,
    /// `$finish`.
    Finish(usize),
}

/// The simulator of one elaborated design.
pub struct Simulator {
    design: Arc<Design>,
    store: Vec<Value4>,
    now: u64,
    seq: u64,
    pc: Vec<u32>,
    waiting: Vec<bool>,
    epoch: Vec<u32>,
    done: Vec<bool>,
    queued: Vec<bool>,
    active: VecDeque<u32>,
    nba: Vec<Nba>,
    waiters: Vec<Vec<Waiter>>,
    timed: BinaryHeap<Reverse<(u64, u64, u32)>>,
    finished: bool,
    log: Vec<String>,
    pending: String,
    recording: bool,
    changes: Vec<Change>,
    /// Values when recording started, by signal (the VCD `$dumpvars`).
    rec_init: Vec<Value4>,
    rec_time: u64,
    /// Drivers of the nets: one per continuous assignment that drives a bit of the net,
    /// and one external driver per net (written by [`Simulator::set4`]).
    drivers: Vec<Driver>,
    /// Per signal, the drivers that resolve it (empty for variables).
    net_drivers: Vec<Vec<u32>>,
    /// Per process, the (net, driver) pairs it drives.
    proc_drivers: Vec<Vec<(SigId, u32)>>,
    /// Per signal, its external driver (`u32::MAX` for a variable).
    ext_driver: Vec<u32>,
}

/// Sign-extends the low `w` bits of `v`.
pub(crate) fn sext(v: u128, w: u32) -> i128 {
    if w == 0 {
        0
    } else if w >= 128 {
        v as i128
    } else if (v >> (w - 1)) & 1 == 1 {
        (v | !mask(w)) as i128
    } else {
        v as i128
    }
}

fn to_i64(v: u128) -> i64 {
    i64::try_from(v).unwrap_or(i64::MIN)
}

/// The value of one bit state as a 1-bit value.
fn bit_value(b: Bit4) -> Value4 {
    match b {
        Bit4::Zero => Value4::known(0),
        Bit4::One => Value4::known(1),
        Bit4::X => Value4::all_x(1),
        Bit4::Z => Value4::all_z(1),
    }
}

/// Edge tests on one bit: a rise is 0 to 1, 0 to x or z, x or z to 1; a fall is the
/// dual (IEEE 1364, 9.7.3).
fn rises(old: Bit4, new: Bit4) -> bool {
    (old == Bit4::Zero && new != Bit4::Zero)
        || (matches!(old, Bit4::X | Bit4::Z) && new == Bit4::One)
}

fn falls(old: Bit4, new: Bit4) -> bool {
    (old == Bit4::One && new != Bit4::One)
        || (matches!(old, Bit4::X | Bit4::Z) && new == Bit4::Zero)
}

impl Simulator {
    /// Creates a simulator. A design with diagnostics is refused, and the diagnostics
    /// are returned.
    pub fn new(design: Design) -> Result<Simulator, Vec<Diagnostic>> {
        if !design.diagnostics.is_empty() {
            return Err(design.diagnostics.clone());
        }
        let n = design.procs.len();
        let nsig = design.signals.len();
        let mut store = vec![Value4::default(); design.store_len];
        for s in &design.signals {
            let init = match s.kind {
                SigKind::Net => Value4::all_z(s.width),
                SigKind::Variable => Value4::all_x(s.width),
                SigKind::TwoState => Value4::known(0),
            };
            for k in 0..s.depth.max(1) as usize {
                store[s.base + k] = init;
            }
        }
        // Drivers: an external driver for every net, then one per (continuous
        // assignment, net) pair, in process order.
        let mut drivers = Vec::new();
        let mut net_drivers: Vec<Vec<u32>> = vec![Vec::new(); nsig];
        let mut ext_driver = vec![u32::MAX; nsig];
        let is_net = |s: &crate::ir::Signal| s.kind == SigKind::Net && s.depth == 0;
        for (i, s) in design.signals.iter().enumerate() {
            if is_net(s) {
                let d = drivers.len() as u32;
                drivers.push(Driver {
                    sig: i as SigId,
                    val: Value4::default(),
                    cover: 0,
                });
                net_drivers[i].push(d);
                ext_driver[i] = d;
            }
        }
        let mut proc_drivers: Vec<Vec<(SigId, u32)>> = vec![Vec::new(); n];
        for (p, proc) in design.procs.iter().enumerate() {
            if proc.kind != ProcKind::Assign {
                continue;
            }
            for op in &proc.code {
                let Op::Assign { lhs, .. } = op else { continue };
                for part in &lhs.parts {
                    let s = &design.signals[part.sig as usize];
                    if !is_net(s) || proc_drivers[p].iter().any(|(q, _)| *q == part.sig) {
                        continue;
                    }
                    let d = drivers.len() as u32;
                    drivers.push(Driver {
                        sig: part.sig,
                        val: Value4::default(),
                        cover: 0,
                    });
                    net_drivers[part.sig as usize].push(d);
                    proc_drivers[p].push((part.sig, d));
                }
            }
        }
        let mut sim = Simulator {
            store,
            now: 0,
            seq: 0,
            pc: vec![0; n],
            waiting: vec![false; n],
            epoch: vec![0; n],
            done: vec![false; n],
            queued: vec![true; n],
            active: (0..n as u32).collect(),
            nba: Vec::new(),
            waiters: vec![Vec::new(); nsig],
            timed: BinaryHeap::new(),
            finished: false,
            log: Vec::new(),
            pending: String::new(),
            recording: false,
            changes: Vec::new(),
            rec_init: Vec::new(),
            rec_time: 0,
            drivers,
            net_drivers,
            proc_drivers,
            ext_driver,
            design: Arc::new(design),
        };
        // Continuous assigns and always_comb start at time 0 with their inputs already
        // registered, so a change made by their own first evaluation is seen.
        for p in 0..n {
            let kind = sim.design.procs[p].kind;
            if matches!(kind, ProcKind::Assign | ProcKind::Comb) {
                let trig = match sim.design.procs[p].code.first() {
                    Some(Op::Wait(t)) => Some(t.clone()),
                    _ => None,
                };
                if let Some(t) = trig {
                    sim.register(p, &t);
                    sim.pc[p] = 1;
                }
            }
        }
        // Time 0: every process runs to its first wait, as in a simulator's start-up.
        sim.settle().map_err(|e| {
            vec![Diagnostic::new(
                &Pos::new("<simulation>", 0, 0),
                "simulation",
                e.to_string(),
            )]
        })?;
        Ok(sim)
    }

    /// The design being simulated.
    pub fn design(&self) -> &Design {
        &self.design
    }

    /// Current time in picoseconds.
    pub fn now_ps(&self) -> u64 {
        self.now
    }

    /// Whether `$finish` has run.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Lines written by `$display`, `$write` and the other system tasks.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Names of the visible signals, sorted.
    pub fn signal_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.design.names.keys().cloned().collect();
        v.sort();
        v
    }

    /// Width of a visible signal.
    pub fn width_of(&self, name: &str) -> Option<u32> {
        self.design
            .signal(name)
            .map(|s| self.design.signals[s as usize].width)
    }

    /// Value of a scalar or vector signal as 2 states: X and Z read as 0 (`None` for an
    /// unknown name or a memory). See [`Simulator::get4`] for the 4-state value.
    pub fn get(&self, name: &str) -> Option<u128> {
        self.get4(name).map(|v| v.to_two_state())
    }

    /// 4-state value of a scalar or vector signal (`None` for an unknown name or a
    /// memory).
    pub fn get4(&self, name: &str) -> Option<Value4> {
        let id = self.design.signal(name)?;
        let s = &self.design.signals[id as usize];
        if s.depth > 0 {
            return None;
        }
        Some(self.store[s.base].norm(s.width))
    }

    /// Value of word `index` (declared coordinates) of a memory, 2-state.
    pub fn get_word(&self, name: &str, index: i64) -> Option<u128> {
        self.get_word4(name, index).map(|v| v.to_two_state())
    }

    /// 4-state value of word `index` (declared coordinates) of a memory.
    pub fn get_word4(&self, name: &str, index: i64) -> Option<Value4> {
        let id = self.design.signal(name)?;
        let s = &self.design.signals[id as usize];
        if s.depth == 0 {
            return None;
        }
        let pos = index - s.mem_lo;
        if pos < 0 || pos >= s.depth as i64 {
            return None;
        }
        Some(self.store[s.base + pos as usize].norm(s.width))
    }

    /// Sets a visible scalar or vector signal to a known value. The change takes effect
    /// (and wakes its readers) at the next [`Simulator::run_until`].
    pub fn set(&mut self, name: &str, value: u128) -> Result<(), String> {
        self.set4(name, Value4::known(value))
    }

    /// Sets a visible scalar or vector signal to a 4-state value. A net takes the value
    /// as an external driver, resolved with its continuous drivers; a variable takes it
    /// directly (a `bit` or `int` takes X and Z as 0).
    pub fn set4(&mut self, name: &str, value: Value4) -> Result<(), String> {
        let id = self
            .design
            .signal(name)
            .ok_or_else(|| format!("no signal named `{name}`"))?;
        let (base, width, depth, kind) = {
            let s = &self.design.signals[id as usize];
            (s.base, s.width, s.depth, s.kind)
        };
        if depth > 0 {
            return Err(format!("`{name}` is a memory; set one word with set_word"));
        }
        if kind == SigKind::Net {
            let d = self.ext_driver[id as usize] as usize;
            self.drivers[d].val = value.norm(width);
            self.drivers[d].cover = mask(width);
            self.update_net(id);
            return Ok(());
        }
        let old = self.store[base];
        let new = self.coerce(id, value.norm(width));
        if old != new {
            self.store[base] = new;
            self.notify(id, old, new);
            self.record(id, new);
        }
        Ok(())
    }

    /// Sets one word of a memory (declared coordinates), 2-state.
    pub fn set_word(&mut self, name: &str, index: i64, value: u128) -> Result<(), String> {
        self.set_word4(name, index, Value4::known(value))
    }

    /// Sets one word of a memory (declared coordinates) to a 4-state value.
    pub fn set_word4(&mut self, name: &str, index: i64, value: Value4) -> Result<(), String> {
        let id = self
            .design
            .signal(name)
            .ok_or_else(|| format!("no signal named `{name}`"))?;
        let (base, width, depth, mem_lo) = {
            let s = &self.design.signals[id as usize];
            (s.base, s.width, s.depth, s.mem_lo)
        };
        let pos = index - mem_lo;
        if depth == 0 || pos < 0 || pos >= depth as i64 {
            return Err(format!("`{name}[{index}]` is not a word of a memory"));
        }
        let idx = base + pos as usize;
        let old = self.store[idx];
        let new = self.coerce(id, value.norm(width));
        if old != new {
            self.store[idx] = new;
            self.notify(id, old, new);
        }
        Ok(())
    }

    /// Drives a clock signal high then low, settling after each edge, at the current
    /// time (no time passes). Use [`Simulator::run_until`] for timed clocks.
    pub fn tick_clock(&mut self, name: &str) -> Result<(), SimError> {
        self.set(name, 1).map_err(|m| self.err(m))?;
        self.run_until(self.now)?;
        self.set(name, 0).map_err(|m| self.err(m))?;
        self.run_until(self.now)
    }

    /// Records every change of a visible signal (see [`Simulator::changes`]). Turning
    /// recording on fixes the values the waveform starts from.
    pub fn set_recording(&mut self, on: bool) {
        if on && !self.recording {
            self.rec_init = self.store.clone();
            self.rec_time = self.now;
        }
        self.recording = on;
    }

    /// Recorded value changes, in time order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// A VCD waveform of the recorded changes. Values are 4-state: a bit is written as
    /// `0`, `1`, `x` or `z`. The `$dumpvars` block holds the values when recording
    /// started.
    pub fn vcd(&self) -> String {
        let mut out = String::new();
        out.push_str("$timescale 1ps $end\n$scope module top $end\n");
        let mut ids: HashMap<SigId, String> = HashMap::new();
        let mut vars: Vec<SigId> = Vec::new();
        for (i, s) in self.design.signals.iter().enumerate() {
            if s.hidden || s.depth > 0 {
                continue;
            }
            let id = vcd_id(vars.len());
            out.push_str(&format!(
                "$var wire {} {} {} $end\n",
                s.width,
                id,
                s.name.replace(' ', "_")
            ));
            ids.insert(i as SigId, id);
            vars.push(i as SigId);
        }
        out.push_str(&format!(
            "$upscope $end\n$enddefinitions $end\n#{}\n$dumpvars\n",
            self.rec_time
        ));
        for sig in &vars {
            let s = &self.design.signals[*sig as usize];
            let v = self
                .rec_init
                .get(*sig as usize)
                .copied()
                .unwrap_or_default();
            out.push_str(&vcd_value(v, s.width, &ids[sig]));
        }
        out.push_str("$end\n");
        let mut last_t = u64::MAX;
        for c in &self.changes {
            let Some(id) = ids.get(&c.sig) else { continue };
            if c.time_ps != last_t {
                out.push_str(&format!("#{}\n", c.time_ps));
                last_t = c.time_ps;
            }
            let w = self.design.signals[c.sig as usize].width;
            out.push_str(&vcd_value(c.value, w, id));
        }
        out
    }

    fn err(&self, message: impl Into<String>) -> SimError {
        SimError {
            time_ps: self.now,
            message: message.into(),
        }
    }

    fn record(&mut self, sig: SigId, value: Value4) {
        if self.recording && self.design.signals[sig as usize].depth == 0 {
            self.changes.push(Change {
                time_ps: self.now,
                sig,
                value,
            });
        }
    }

    /// Runs the simulation until time `t_ps` (picoseconds). Returns early when
    /// `$finish` runs.
    pub fn run_until(&mut self, t_ps: u64) -> Result<(), SimError> {
        loop {
            self.settle()?;
            if self.finished {
                return Ok(());
            }
            match self.timed.peek() {
                Some(Reverse((t, _, _))) if *t <= t_ps => {
                    let t = *t;
                    self.now = t;
                    while let Some(Reverse((tt, _, p))) = self.timed.peek().copied() {
                        if tt != t {
                            break;
                        }
                        self.timed.pop();
                        self.wake_timed(p);
                    }
                }
                _ => {
                    if t_ps > self.now {
                        self.now = t_ps;
                    }
                    return Ok(());
                }
            }
        }
    }

    fn wake_timed(&mut self, p: u32) {
        let pi = p as usize;
        if !self.done[pi] && !self.queued[pi] {
            self.queued[pi] = true;
            self.active.push_back(p);
        }
    }

    /// Runs the active and non-blocking regions until both are empty.
    fn settle(&mut self) -> Result<(), SimError> {
        let mut activations = 0u64;
        let mut rounds = 0u64;
        loop {
            while let Some(p) = self.active.pop_front() {
                self.queued[p as usize] = false;
                activations += 1;
                if activations > ACTIVATION_LIMIT {
                    return Err(self.err(
                        "combinational loop: too many process activations in one time step (a signal feeds itself without a clock?)",
                    ));
                }
                self.run_proc(p as usize)?;
                if self.finished {
                    return Ok(());
                }
            }
            if self.nba.is_empty() {
                return Ok(());
            }
            rounds += 1;
            if rounds > DELTA_LIMIT {
                return Err(self.err(
                    "combinational loop: non-blocking updates never settle in one time step",
                ));
            }
            let batch = std::mem::take(&mut self.nba);
            for u in batch {
                self.write_bits(u.sig, u.idx, u.lo, u.w, u.value);
            }
        }
    }

    fn run_proc(&mut self, p: usize) -> Result<(), SimError> {
        if self.done[p] {
            return Ok(());
        }
        let design = self.design.clone();
        let pc0 = self.pc[p] as usize;
        let scope = &design.procs[p].scope;
        match self.exec(&design.procs[p].code, pc0, Some(p), scope)? {
            Exit::Wait(next) => {
                self.pc[p] = next as u32;
            }
            Exit::Delay(next) => {
                // The wake-up is already queued by `exec`.
                self.pc[p] = next as u32;
            }
            Exit::Halt => {
                self.done[p] = true;
            }
            Exit::Finish(next) => {
                self.pc[p] = next as u32;
                self.finished = true;
            }
        }
        Ok(())
    }

    /// Executes bytecode from `pc0`. `proc` is the process being run (`None` for a
    /// function, which may not suspend).
    fn exec(
        &mut self,
        code: &[Op],
        pc0: usize,
        proc: Option<usize>,
        scope: &str,
    ) -> Result<Exit, SimError> {
        let mut pc = pc0;
        let mut budget = STEP_BUDGET;
        loop {
            if budget == 0 {
                return Err(self.err(format!(
                    "step budget exceeded in `{scope}`: a loop with no timing control?"
                )));
            }
            budget -= 1;
            let Some(op) = code.get(pc) else {
                return Ok(Exit::Halt);
            };
            match op {
                Op::Assign { lhs, rhs, nb } => {
                    let v = self.eval(rhs)?;
                    // Continuous assignments drive nets through their drivers.
                    let drive = proc.filter(|p| self.design.procs[*p].kind == ProcKind::Assign);
                    self.assign(lhs, v, *nb, drive)?;
                    pc += 1;
                }
                Op::Jump(t) => pc = *t as usize,
                Op::JumpIf { cond, to } => {
                    pc = if self.eval(cond)?.truth() == Some(true) {
                        *to as usize
                    } else {
                        pc + 1
                    };
                }
                Op::JumpIfNot { cond, to } => {
                    pc = if self.eval(cond)?.truth() != Some(true) {
                        *to as usize
                    } else {
                        pc + 1
                    };
                }
                Op::Delay { amount, unit_ps } => {
                    let Some(_) = proc else {
                        return Err(self.err(format!("a delay inside the function of `{scope}`")));
                    };
                    // An unknown delay is taken as 0.
                    let n = self.eval(amount)?.bits;
                    let at = self.now.saturating_add((n as u64).saturating_mul(*unit_ps));
                    self.seq += 1;
                    let p = proc.unwrap_or(0) as u32;
                    self.timed.push(Reverse((at, self.seq, p)));
                    return Ok(Exit::Delay(pc + 1));
                }
                Op::Wait(triggers) => {
                    let Some(p) = proc else {
                        return Err(
                            self.err(format!("an event control inside the function of `{scope}`"))
                        );
                    };
                    self.register(p, triggers);
                    return Ok(Exit::Wait(pc + 1));
                }
                Op::Sys(s) => {
                    let finish = self.sys(s, scope)?;
                    pc += 1;
                    if finish {
                        return Ok(Exit::Finish(pc));
                    }
                }
                Op::Halt => return Ok(Exit::Halt),
            }
        }
    }

    fn register(&mut self, p: usize, triggers: &[Trigger]) {
        self.waiting[p] = true;
        let epoch = self.epoch[p];
        for t in triggers {
            self.waiters[t.sig as usize].push(Waiter {
                proc: p as u32,
                epoch,
                bit: t.bit,
                edge: t.edge,
            });
        }
    }

    /// Wakes the processes whose trigger fires on a change of `sig`.
    fn notify(&mut self, sig: SigId, old: Value4, new: Value4) {
        let list = std::mem::take(&mut self.waiters[sig as usize]);
        let mut keep = Vec::with_capacity(list.len());
        let mut fired = Vec::new();
        for w in list {
            let p = w.proc as usize;
            if !self.waiting[p] || self.epoch[p] != w.epoch {
                continue; // stale registration
            }
            let b = w.bit.unwrap_or(0);
            let (ob, nb) = (old.bit(b), new.bit(b));
            let fire = match w.edge {
                Edge::Any => match w.bit {
                    None => old != new,
                    Some(_) => ob != nb,
                },
                Edge::Pos => rises(ob, nb),
                Edge::Neg => falls(ob, nb),
            };
            if fire {
                fired.push(p);
            } else {
                keep.push(w);
            }
        }
        self.waiters[sig as usize] = keep;
        for p in fired {
            self.waiting[p] = false;
            self.epoch[p] = self.epoch[p].wrapping_add(1);
            if !self.done[p] && !self.queued[p] {
                self.queued[p] = true;
                self.active.push_back(p as u32);
            }
        }
    }

    /// The value a variable of this signal holds after a write: a `bit` or `int` has
    /// no X or Z.
    fn coerce(&self, sig: SigId, v: Value4) -> Value4 {
        if self.design.signals[sig as usize].kind == SigKind::TwoState {
            Value4::known(v.bits)
        } else {
            v
        }
    }

    /// Stores `value` into `w` bits of word `idx` of `sig` at bit `lo`.
    fn write_bits(&mut self, sig: SigId, idx: usize, lo: u32, w: u32, value: Value4) {
        let old = self.store[idx];
        let new = self.coerce(sig, old.insert(lo, w, value));
        if new != old {
            self.store[idx] = new;
            self.notify(sig, old, new);
            if self.design.signals[sig as usize].depth == 0 {
                self.record(sig, new);
            }
        }
    }

    /// Recomputes a net from its drivers and propagates a change.
    fn update_net(&mut self, sig: SigId) {
        let s = &self.design.signals[sig as usize];
        let (base, width) = (s.base, s.width);
        let new = resolve(
            self.net_drivers[sig as usize]
                .iter()
                .map(|&d| (self.drivers[d as usize].val, self.drivers[d as usize].cover)),
            width,
        );
        let old = self.store[base];
        if new != old {
            self.store[base] = new;
            self.notify(sig, old, new);
            self.record(sig, new);
        }
    }

    fn assign(
        &mut self,
        lhs: &LVal,
        v: Value4,
        nb: bool,
        drive: Option<usize>,
    ) -> Result<(), SimError> {
        let mut offset = lhs.w;
        // Drivers of a continuous assignment: every one of this process starts with no
        // bits, and the bits it writes now are added.
        let mut touched: Vec<(u32, Value4, u128)> = match drive {
            Some(p) => self.proc_drivers[p]
                .iter()
                .map(|&(_, d)| (d, Value4::default(), 0))
                .collect(),
            None => Vec::new(),
        };
        for part in &lhs.parts {
            offset -= part.w;
            let pv = v.part(offset, part.w);
            let word = match &part.word {
                Some(ex) => {
                    let i = self.eval(ex)?;
                    if !i.is_known() {
                        continue;
                    }
                    let pos = to_i64(i.bits) - part.word_off;
                    let depth = self.design.signals[part.sig as usize].depth as i64;
                    if pos < 0 || pos >= depth {
                        continue;
                    }
                    pos as usize
                }
                None => 0,
            };
            let lo = match &part.sel {
                None => 0,
                Some((start, off, flip, up)) => {
                    let st = self.eval(start)?;
                    if !st.is_known() {
                        continue;
                    }
                    let s = to_i64(st.bits);
                    let w = part.w as i64;
                    let a = if *up { s } else { s - (w - 1) };
                    let p = if *flip { off - (a + w - 1) } else { a - off };
                    let width = self.design.signals[part.sig as usize].width as i64;
                    if p < 0 || p + w > width {
                        continue;
                    }
                    p as u32
                }
            };
            if let Some(p) = drive {
                let is_net = {
                    let s = &self.design.signals[part.sig as usize];
                    s.kind == SigKind::Net && s.depth == 0
                };
                if is_net {
                    let d = self.proc_drivers[p]
                        .iter()
                        .find(|(s, _)| *s == part.sig)
                        .map(|(_, d)| *d);
                    if let Some(d) = d {
                        let placed = pv.shifted(lo, part.w);
                        let cover = mask(part.w) << lo;
                        let e = touched
                            .iter_mut()
                            .find(|(id, _, _)| *id == d)
                            .expect("a driver of this process");
                        e.1 = Value4 {
                            bits: e.1.bits | placed.bits,
                            unknown: e.1.unknown | placed.unknown,
                            z: e.1.z | placed.z,
                        };
                        e.2 |= cover;
                        continue;
                    }
                }
            }
            let idx = self.design.signals[part.sig as usize].base + word;
            if nb {
                self.nba.push(Nba {
                    idx,
                    sig: part.sig,
                    lo,
                    w: part.w,
                    value: pv,
                });
            } else {
                self.write_bits(part.sig, idx, lo, part.w, pv);
            }
        }
        if drive.is_some() && !touched.is_empty() {
            let mut nets: Vec<SigId> = Vec::new();
            for (d, val, cover) in touched {
                let dr = &mut self.drivers[d as usize];
                dr.val = val;
                dr.cover = cover;
                if !nets.contains(&dr.sig) {
                    nets.push(dr.sig);
                }
            }
            for n in nets {
                self.update_net(n);
            }
        }
        Ok(())
    }

    /// Evaluates an expression to its `w`-bit value.
    fn eval(&mut self, e: &Ex) -> Result<Value4, SimError> {
        let w = e.w;
        let m = mask(w);
        Ok(match &e.k {
            K::Const(v) => Value4::known(v & m),
            K::Lit(v) => v.norm(w),
            K::Sig(id) => {
                let s = &self.design.signals[*id as usize];
                self.store[s.base].norm(w)
            }
            K::Word { sig, idx, off } => {
                let i = self.eval(idx)?;
                let s = &self.design.signals[*sig as usize];
                let pos = to_i64(i.bits) - off;
                if !i.is_known() || pos < 0 || pos >= s.depth as i64 {
                    Value4::all_x(w)
                } else {
                    self.store[s.base + pos as usize].norm(w)
                }
            }
            K::Slice {
                base,
                start,
                off,
                flip,
                up,
            } => {
                let bv = self.eval(base)?;
                let st = self.eval(start)?;
                if !st.is_known() {
                    return Ok(Value4::all_x(w));
                }
                let s = to_i64(st.bits);
                let wi = w as i64;
                let a = if *up { s } else { s - (wi - 1) };
                let p = if *flip { off - (a + wi - 1) } else { a - off };
                let bw = base.w as i64;
                if p >= 0 && p + wi <= bw {
                    bv.part(p as u32, w)
                } else {
                    // Bits outside the vector read as X.
                    let mut r = Value4::default();
                    for k in 0..wi {
                        let q = p + k;
                        let b = if q >= 0 && q < bw {
                            bv.bit(q as u32)
                        } else {
                            Bit4::X
                        };
                        r = r.insert(k as u32, 1, bit_value(b));
                    }
                    r
                }
            }
            K::Un(op, a) => {
                let x = self.eval(a)?;
                unary(*op, x, a.w, w)
            }
            K::Bin { op, a, b, os } => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                bin(*op, x, y, a.w, b.w, w, *os)
            }
            K::Tern(c, t, f) => {
                let cv = self.eval(c)?;
                match cv.truth() {
                    Some(true) => self.eval(t)?,
                    Some(false) => self.eval(f)?,
                    None => {
                        let tv = self.eval(t)?;
                        let fv = self.eval(f)?;
                        merge(tv, fv, m)
                    }
                }
            }
            K::Concat(parts) => {
                let mut acc = Value4::default();
                for p in parts {
                    let v = self.eval(p)?;
                    acc = acc.push(v, p.w);
                }
                acc.norm(w)
            }
            K::Ext(a) => {
                let v = self.eval(a)?;
                extend(v, a.w, w, e.s)
            }
            K::Trunc(a) => self.eval(a)?.norm(w),
            K::Call { fid, args } => self.call(*fid as usize, args)?,
            K::Time(unit) => {
                let u = (*unit).max(1);
                Value4::known((self.now / u) as u128 & m)
            }
            K::CaseEq { a, b, mode } => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                Value4::known(case_match(x, y, mask(a.w), *mode) as u128)
            }
        })
    }

    fn call(&mut self, fid: usize, args: &[Ex]) -> Result<Value4, SimError> {
        let design = self.design.clone();
        let f: &Func = &design.funcs[fid];
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.eval(a)?);
        }
        for (formal, v) in f.formals.iter().zip(vals) {
            let s = &design.signals[*formal as usize];
            let base = s.base;
            let w = s.width;
            let old = self.store[base];
            let new = v.norm(w);
            self.store[base] = new;
            if old != new {
                self.notify(*formal, old, new);
            }
        }
        match self.exec(&f.code, 0, None, &f.name)? {
            Exit::Halt | Exit::Wait(_) | Exit::Delay(_) | Exit::Finish(_) => {}
        }
        let r = &design.signals[f.ret as usize];
        Ok(self.store[r.base].norm(r.width))
    }

    /// Runs a system task. Returns `true` for `$finish`.
    fn sys(&mut self, s: &Sys, scope: &str) -> Result<bool, SimError> {
        match s {
            Sys::Display {
                args,
                newline,
                scope: sc,
            } => {
                let text = self.format(args, if sc.is_empty() { scope } else { sc })?;
                let line = format!("{}{}", std::mem::take(&mut self.pending), text);
                if *newline {
                    self.log.push(line);
                } else {
                    self.pending = line;
                }
                Ok(false)
            }
            Sys::Finish => Ok(true),
            Sys::ReadMem { sig, file, hex } => {
                self.read_mem(*sig, file, *hex)?;
                Ok(false)
            }
        }
    }

    fn format(&mut self, args: &[Arg], scope: &str) -> Result<String, SimError> {
        let (fmt, rest) = match args.first() {
            Some(Arg::Str(f)) => (f.clone(), &args[1..]),
            _ => (String::new(), args),
        };
        let mut out = String::new();
        let mut vals = Vec::new();
        if args.first().is_some_and(|a| !matches!(a, Arg::Str(_))) || fmt.is_empty() {
            // No format string: decimal values separated by spaces.
            for (i, a) in rest.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                match a {
                    Arg::Str(s) => out.push_str(s),
                    Arg::Val(e, sg) => {
                        let v = self.eval(e)?;
                        out.push_str(&decimal(v, e.w, *sg));
                    }
                }
            }
            return Ok(out);
        }
        for a in rest {
            vals.push(match a {
                Arg::Str(s) => Err(s.clone()),
                Arg::Val(e, sg) => Ok((self.eval(e)?, e.w, *sg)),
            });
        }
        let chars: Vec<char> = fmt.chars().collect();
        let mut i = 0;
        let mut next = 0usize;
        while i < chars.len() {
            let c = chars[i];
            i += 1;
            if c != '%' {
                out.push(c);
                continue;
            }
            let mut zero = false;
            let mut width: Option<usize> = None;
            while i < chars.len() && chars[i].is_ascii_digit() {
                if chars[i] == '0' && width.is_none() {
                    zero = true;
                } else {
                    width =
                        Some(width.unwrap_or(0) * 10 + chars[i].to_digit(10).unwrap_or(0) as usize);
                }
                i += 1;
            }
            let Some(&conv) = chars.get(i) else { break };
            i += 1;
            if conv == '%' {
                out.push('%');
                continue;
            }
            if conv == 'm' || conv == 'M' {
                out.push_str(scope);
                continue;
            }
            let arg = vals.get(next).cloned();
            next += 1;
            let piece = match (conv, arg) {
                (_, None) => String::new(),
                (_, Some(Err(s))) => s,
                ('t' | 'T', Some(Ok(_))) => (self.now).to_string(),
                ('c' | 'C', Some(Ok((v, _, _)))) => ((v.bits as u8) as char).to_string(),
                ('s' | 'S', Some(Ok((v, w, _)))) => {
                    let mut s = String::new();
                    for k in (0..w.div_ceil(8)).rev() {
                        let b = ((v.bits >> (k * 8)) & 0xFF) as u8;
                        if b != 0 {
                            s.push(b as char);
                        }
                    }
                    s
                }
                ('h' | 'H' | 'x' | 'X', Some(Ok((v, w, _)))) => {
                    pad_digits(radix_text(v, w, 4), width, zero)
                }
                ('b' | 'B', Some(Ok((v, w, _)))) => pad_digits(radix_text(v, w, 1), width, zero),
                ('o' | 'O', Some(Ok((v, w, _)))) => pad_digits(radix_text(v, w, 3), width, zero),
                (_, Some(Ok((v, w, sg)))) => {
                    let s = decimal(v, w, sg);
                    let field = if zero {
                        None
                    } else {
                        Some(max_decimal_digits(w, sg))
                    };
                    pad(&s, width.or(field), false, ' ')
                }
            };
            out.push_str(&piece);
        }
        Ok(out)
    }

    fn read_mem(&mut self, sig: SigId, file: &str, hex: bool) -> Result<(), SimError> {
        let design = self.design.clone();
        let s = &design.signals[sig as usize];
        let path = match &design.base_dir {
            Some(d) => d.join(file),
            None => std::path::PathBuf::from(file),
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| self.err(format!("$readmem: cannot read `{}`: {e}", path.display())))?;
        let text = strip_comments(&text);
        let mut addr: i64 = 0;
        for tok in text.split_whitespace() {
            if let Some(a) = tok.strip_prefix('@') {
                addr = i64::from_str_radix(a, 16)
                    .map_err(|_| self.err(format!("$readmem: bad address `{tok}`")))?;
                continue;
            }
            let digits: String = tok.chars().filter(|c| *c != '_').collect();
            let v = radix_bits(
                &digits.to_ascii_lowercase(),
                if hex { 4 } else { 1 },
                s.width,
            );
            let pos = addr - s.mem_lo;
            if pos >= 0 && pos < s.depth as i64 {
                let idx = s.base + pos as usize;
                let old = self.store[idx];
                let new = self.coerce(sig, v);
                if old != new {
                    self.store[idx] = new;
                    self.notify(sig, old, new);
                }
            }
            addr += 1;
        }
        Ok(())
    }
}

const SNAP_MAGIC: &[u8; 8] = b"RTLENG02";

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).ok_or("snapshot length overflow")?;
        let s = self.data.get(self.at..end).ok_or("snapshot is truncated")?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "bad u32")?,
        ))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| "bad u64")?,
        ))
    }
    fn u128(&mut self) -> Result<u128, String> {
        Ok(u128::from_le_bytes(
            self.take(16)?.try_into().map_err(|_| "bad u128")?,
        ))
    }
    fn value(&mut self) -> Result<Value4, String> {
        Ok(Value4 {
            bits: self.u128()?,
            unknown: self.u128()?,
            z: self.u128()?,
        })
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u64()? as usize;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| "snapshot string is not UTF-8".to_string())
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u128(out: &mut Vec<u8>, v: u128) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_value(out: &mut Vec<u8>, v: Value4) {
    put_u128(out, v.bits);
    put_u128(out, v.unknown);
    put_u128(out, v.z);
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u64(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn edge_code(e: Edge) -> u8 {
    match e {
        Edge::Any => 0,
        Edge::Pos => 1,
        Edge::Neg => 2,
    }
}

fn edge_from(c: u8) -> Result<Edge, String> {
    match c {
        0 => Ok(Edge::Any),
        1 => Ok(Edge::Pos),
        2 => Ok(Edge::Neg),
        _ => Err("bad edge in snapshot".into()),
    }
}

impl Simulator {
    /// Serialises the whole simulation state: values (4-state), drivers, process
    /// positions and waits, queues, timed events, the log. [`Simulator::restore`] reads
    /// it back into a simulator of the same design.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SNAP_MAGIC);
        put_u64(&mut out, self.design.store_len as u64);
        put_u64(&mut out, self.design.procs.len() as u64);
        put_u64(&mut out, self.design.signals.len() as u64);
        put_u64(&mut out, self.now);
        put_u64(&mut out, self.seq);
        out.push(self.finished as u8);
        put_u64(&mut out, self.store.len() as u64);
        for v in &self.store {
            put_value(&mut out, *v);
        }
        put_u64(&mut out, self.drivers.len() as u64);
        for d in &self.drivers {
            put_value(&mut out, d.val);
            put_u128(&mut out, d.cover);
        }
        for p in 0..self.pc.len() {
            put_u32(&mut out, self.pc[p]);
            out.push(self.waiting[p] as u8);
            put_u32(&mut out, self.epoch[p]);
            out.push(self.done[p] as u8);
            out.push(self.queued[p] as u8);
        }
        put_u64(&mut out, self.active.len() as u64);
        for p in &self.active {
            put_u32(&mut out, *p);
        }
        put_u64(&mut out, self.nba.len() as u64);
        for u in &self.nba {
            put_u64(&mut out, u.idx as u64);
            put_u32(&mut out, u.sig);
            put_u32(&mut out, u.lo);
            put_u32(&mut out, u.w);
            put_value(&mut out, u.value);
        }
        put_u64(&mut out, self.waiters.len() as u64);
        for list in &self.waiters {
            put_u64(&mut out, list.len() as u64);
            for w in list {
                put_u32(&mut out, w.proc);
                put_u32(&mut out, w.epoch);
                put_u32(&mut out, w.bit.unwrap_or(u32::MAX));
                out.push(edge_code(w.edge));
            }
        }
        let mut timed: Vec<(u64, u64, u32)> = self.timed.iter().map(|Reverse(t)| *t).collect();
        timed.sort_unstable();
        put_u64(&mut out, timed.len() as u64);
        for (t, s, p) in timed {
            put_u64(&mut out, t);
            put_u64(&mut out, s);
            put_u32(&mut out, p);
        }
        put_u64(&mut out, self.log.len() as u64);
        for l in &self.log {
            put_str(&mut out, l);
        }
        put_str(&mut out, &self.pending);
        out
    }

    /// Replaces the state with a snapshot taken from a simulator of the same design.
    /// The change record is cleared.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = Reader { data: bytes, at: 0 };
        if r.take(SNAP_MAGIC.len())? != SNAP_MAGIC {
            return Err("not a simulator snapshot".into());
        }
        let store_len = r.u64()? as usize;
        let nprocs = r.u64()? as usize;
        let nsig = r.u64()? as usize;
        if store_len != self.design.store_len
            || nprocs != self.design.procs.len()
            || nsig != self.design.signals.len()
        {
            return Err("snapshot belongs to a different design".into());
        }
        self.now = r.u64()?;
        self.seq = r.u64()?;
        self.finished = r.u8()? != 0;
        let n = r.u64()? as usize;
        if n != store_len {
            return Err("snapshot store has the wrong size".into());
        }
        for i in 0..n {
            self.store[i] = r.value()?;
        }
        let nd = r.u64()? as usize;
        if nd != self.drivers.len() {
            return Err("snapshot has the wrong number of drivers".into());
        }
        for i in 0..nd {
            self.drivers[i].val = r.value()?;
            self.drivers[i].cover = r.u128()?;
        }
        for p in 0..nprocs {
            self.pc[p] = r.u32()?;
            self.waiting[p] = r.u8()? != 0;
            self.epoch[p] = r.u32()?;
            self.done[p] = r.u8()? != 0;
            self.queued[p] = r.u8()? != 0;
        }
        let na = r.u64()? as usize;
        self.active.clear();
        for _ in 0..na {
            self.active.push_back(r.u32()?);
        }
        let nn = r.u64()? as usize;
        self.nba.clear();
        for _ in 0..nn {
            let idx = r.u64()? as usize;
            let sig = r.u32()?;
            let lo = r.u32()?;
            let w = r.u32()?;
            let value = r.value()?;
            self.nba.push(Nba {
                idx,
                sig,
                lo,
                w,
                value,
            });
        }
        let nw = r.u64()? as usize;
        if nw != nsig {
            return Err("snapshot waiter table has the wrong size".into());
        }
        for i in 0..nw {
            let k = r.u64()? as usize;
            let mut list = Vec::with_capacity(k);
            for _ in 0..k {
                let proc = r.u32()?;
                let epoch = r.u32()?;
                let bit = match r.u32()? {
                    u32::MAX => None,
                    b => Some(b),
                };
                let edge = edge_from(r.u8()?)?;
                list.push(Waiter {
                    proc,
                    epoch,
                    bit,
                    edge,
                });
            }
            self.waiters[i] = list;
        }
        let nt = r.u64()? as usize;
        self.timed.clear();
        for _ in 0..nt {
            let t = r.u64()?;
            let s = r.u64()?;
            let p = r.u32()?;
            self.timed.push(Reverse((t, s, p)));
        }
        let nl = r.u64()? as usize;
        self.log.clear();
        for _ in 0..nl {
            self.log.push(r.string()?);
        }
        self.pending = r.string()?;
        if r.at != bytes.len() {
            return Err("snapshot has trailing bytes".into());
        }
        self.changes.clear();
        Ok(())
    }

    /// FNV-1a hash of the snapshot: equal states give equal hashes. The hash covers the
    /// unknown and Z masks as well as the values.
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.snapshot() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Unary operator on a value of width `aw`, giving `w` bits.
fn unary(op: UnOp, x: Value4, aw: u32, w: u32) -> Value4 {
    let m = mask(w);
    match op {
        UnOp::Plus => x.norm(w),
        UnOp::Minus => arith(Value4::default(), x, m, |_, b| 0u128.wrapping_sub(b)),
        UnOp::BitNot => not(x, m),
        UnOp::LNot => lnot(x),
        UnOp::RedAnd => reduce_and(x, aw),
        UnOp::RedNand => not(reduce_and(x, aw), 1),
        UnOp::RedOr => reduce_or(x, aw),
        UnOp::RedNor => not(reduce_or(x, aw), 1),
        UnOp::RedXor => reduce_xor(x, aw),
        UnOp::RedXnor => not(reduce_xor(x, aw), 1),
    }
}

/// Binary operator on operands of widths `aw` and `bw`, giving `w` bits.
fn bin(op: BinOp, x: Value4, y: Value4, aw: u32, bw: u32, w: u32, os: bool) -> Value4 {
    let m = mask(w);
    match op {
        BinOp::Add => arith(x, y, m, |a, b| a.wrapping_add(b)),
        BinOp::Sub => arith(x, y, m, |a, b| a.wrapping_sub(b)),
        BinOp::Mul => arith(x, y, m, |a, b| a.wrapping_mul(b)),
        BinOp::Div | BinOp::Mod => {
            // An unknown operand, or a zero divisor, gives X (IEEE 1364, 5.1.4).
            if (x.unknown | y.unknown) != 0 || y.bits == 0 {
                return Value4::all_x(w);
            }
            if os {
                let (a, b) = (sext(x.bits, w), sext(y.bits, w));
                let r = if op == BinOp::Div {
                    a.wrapping_div(b)
                } else {
                    a.wrapping_rem(b)
                };
                Value4::known((r as u128) & m)
            } else if op == BinOp::Div {
                Value4::known((x.bits / y.bits) & m)
            } else {
                Value4::known((x.bits % y.bits) & m)
            }
        }
        BinOp::Pow => arith(x, y, m, |a, b| pow_bits(a, b, w, bw, os, m)),
        BinOp::And => and(x, y, m),
        BinOp::Or => or(x, y, m),
        BinOp::Xor => xor(x, y, m),
        BinOp::Xnor => not(xor(x, y, m), m),
        BinOp::Shl | BinOp::AShl => match y.truth_known() {
            None => Value4::all_x(w),
            Some(n) => shl(x, n, m),
        },
        BinOp::Shr => match y.truth_known() {
            None => Value4::all_x(w),
            Some(n) => shr(x, n, w),
        },
        BinOp::AShr => match y.truth_known() {
            None => Value4::all_x(w),
            Some(n) => {
                if os {
                    ashr(x, n, w)
                } else {
                    shr(x, n, w)
                }
            }
        },
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if (x.unknown | y.unknown) != 0 {
                return Value4::all_x(1);
            }
            let ord = if os {
                sext(x.bits, aw).cmp(&sext(y.bits, aw))
            } else {
                x.bits.cmp(&y.bits)
            };
            let r = match op {
                BinOp::Lt => ord.is_lt(),
                BinOp::Le => ord.is_le(),
                BinOp::Gt => ord.is_gt(),
                _ => ord.is_ge(),
            };
            Value4::known(r as u128)
        }
        BinOp::Eq => logical_eq(x, y, mask(aw)),
        BinOp::Ne => {
            let r = logical_eq(x, y, mask(aw));
            if r.unknown != 0 {
                r
            } else {
                Value4::known(1 - r.bits)
            }
        }
        BinOp::CaseEq => Value4::known(case_eq(x, y, mask(aw)) as u128),
        BinOp::CaseNe => Value4::known(!case_eq(x, y, mask(aw)) as u128),
        BinOp::LAnd => land(x, y),
        BinOp::LOr => lor(x, y),
    }
}

/// Power on known bits (the caller has checked for unknown operands).
fn pow_bits(x: u128, y: u128, w: u32, bw: u32, os: bool, m: u128) -> u128 {
    if os && sext(y, bw) < 0 {
        let a = sext(x, w);
        return match a {
            1 => 1,
            -1 => {
                if y & 1 == 1 {
                    m
                } else {
                    1
                }
            }
            _ => 0,
        };
    }
    let mut base = x & m;
    let mut e = y;
    let mut r: u128 = 1;
    while e > 0 {
        if e & 1 == 1 {
            r = r.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        e >>= 1;
    }
    r & m
}

impl Value4 {
    /// The value as a shift amount: `None` when any bit is unknown.
    fn truth_known(self) -> Option<u32> {
        if self.unknown != 0 {
            None
        } else {
            Some(self.bits.min(128) as u32)
        }
    }
}

fn decimal(v: Value4, w: u32, signed: bool) -> String {
    let v = v.norm(w);
    if v.unknown != 0 {
        // Any X makes the whole decimal x; all-Z makes it z.
        return if v.unknown & !v.z != 0 {
            "x".to_string()
        } else {
            "z".to_string()
        };
    }
    if signed {
        sext(v.bits, w).to_string()
    } else {
        v.bits.to_string()
    }
}

/// Digits of radix `2^k` (`k` = 1, 3 or 4 for `%b`, `%o`, `%h`), most significant first.
/// A digit group that is all known prints as its digit; an all-X group prints `x`, an
/// all-Z group `z`; a partly unknown group prints `X` when it has an X and `Z` otherwise.
fn radix_text(v: Value4, w: u32, k: u32) -> String {
    let groups = w.div_ceil(k);
    let mut s = String::new();
    for g in (0..groups).rev() {
        let lo = g * k;
        let n = k.min(w - lo);
        let gm = mask(n) << lo;
        let unk = gm & v.unknown;
        let x_bits = gm & v.unknown & !v.z;
        let ch = if unk == 0 {
            let d = (v.bits >> lo) & mask(n);
            char::from_digit(d as u32, 1 << k).unwrap_or('?')
        } else if gm & !v.unknown == 0 {
            // All unknown.
            if x_bits == 0 {
                'z'
            } else if x_bits == gm {
                'x'
            } else {
                'X'
            }
        } else if x_bits != 0 {
            'X'
        } else {
            'Z'
        };
        s.push(ch);
    }
    s
}

/// Pads a radix digit string to `width` with zeros, or with the leading `x` or `z`
/// when the value starts with one.
fn pad_digits(s: String, width: Option<usize>, _zero: bool) -> String {
    match width {
        Some(n) if s.chars().count() < n => {
            let fill = s
                .chars()
                .next()
                .filter(|c| matches!(c, 'x' | 'X' | 'z' | 'Z'))
                .unwrap_or('0');
            let mut out: String = std::iter::repeat_n(fill, n - s.chars().count()).collect();
            out.push_str(&s);
            out
        }
        _ => s,
    }
}

/// Decimal digits needed for the largest `w`-bit value (plus a sign if signed).
fn max_decimal_digits(w: u32, signed: bool) -> usize {
    let max = mask(w);
    let mut digits = max.to_string().len();
    if signed {
        digits += 1;
    }
    digits
}

fn pad(s: &str, width: Option<usize>, zero: bool, fill: char) -> String {
    match width {
        Some(n) if s.len() < n => {
            let f = if zero { '0' } else { fill };
            let mut out: String = std::iter::repeat_n(f, n - s.len()).collect();
            out.push_str(s);
            out
        }
        _ => s.to_string(),
    }
}

/// Removes `//` and `/* */` comments from memory-file text.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'/') {
            for d in chars.by_ref() {
                if d == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for d in chars.by_ref() {
                if prev == '*' && d == '/' {
                    break;
                }
                prev = d;
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

fn vcd_id(n: usize) -> String {
    let mut n = n;
    let mut s = String::new();
    loop {
        s.push((b'!' + (n % 94) as u8) as char);
        n /= 94;
        if n == 0 {
            break;
        }
    }
    s
}

fn vcd_value(v: Value4, w: u32, id: &str) -> String {
    if w == 1 {
        format!("{}{}\n", v.bit(0).ch(), id)
    } else {
        format!("b{} {id}\n", v.to_bin_string(w))
    }
}
