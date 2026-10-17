//! The event-driven simulator.
//!
//! Scheduling follows the Verilog model. At one time step the **active** region runs
//! every ready process until it is empty; then the **non-blocking** queue is applied
//! in order, which may make more processes ready. This repeats until both are empty
//! (a delta cycle is one such round). Timed events (`#n`) move time forward to the
//! next one. A step with too many process activations or too many non-blocking
//! rounds is reported as a combinational loop.
//!
//! Values are 2-state: every signal is a `u128` masked to its width. x and z in source
//! read as 0 (see `docs/RTL_SUBSET.md`).

use crate::design::Design;
use crate::ir::{Arg, Edge, Ex, Func, K, LVal, Op, ProcKind, SigId, Sys, Trigger};
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
    pub value: u128,
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
    value: u128,
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
    store: Vec<u128>,
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
}

/// Mask of the low `w` bits.
pub(crate) fn mask(w: u32) -> u128 {
    if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    }
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

impl Simulator {
    /// Creates a simulator. A design with diagnostics is refused, and the diagnostics
    /// are returned.
    pub fn new(design: Design) -> Result<Simulator, Vec<Diagnostic>> {
        if !design.diagnostics.is_empty() {
            return Err(design.diagnostics.clone());
        }
        let n = design.procs.len();
        let nsig = design.signals.len();
        let store = vec![0u128; design.store_len];
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

    /// Value of a scalar or vector signal (`None` for an unknown name or a memory).
    pub fn get(&self, name: &str) -> Option<u128> {
        let id = self.design.signal(name)?;
        let s = &self.design.signals[id as usize];
        if s.depth > 0 {
            return None;
        }
        Some(self.store[s.base] & mask(s.width))
    }

    /// Value of word `index` (declared coordinates) of a memory.
    pub fn get_word(&self, name: &str, index: i64) -> Option<u128> {
        let id = self.design.signal(name)?;
        let s = &self.design.signals[id as usize];
        if s.depth == 0 {
            return None;
        }
        let pos = index - s.mem_lo;
        if pos < 0 || pos >= s.depth as i64 {
            return None;
        }
        Some(self.store[s.base + pos as usize] & mask(s.width))
    }

    /// Sets a visible scalar or vector signal. The change takes effect (and wakes its
    /// readers) at the next [`Simulator::run_until`].
    pub fn set(&mut self, name: &str, value: u128) -> Result<(), String> {
        let id = self
            .design
            .signal(name)
            .ok_or_else(|| format!("no signal named `{name}`"))?;
        let (base, width, depth) = {
            let s = &self.design.signals[id as usize];
            (s.base, s.width, s.depth)
        };
        if depth > 0 {
            return Err(format!("`{name}` is a memory; set one word with set_word"));
        }
        let old = self.store[base];
        let new = value & mask(width);
        if old != new {
            self.store[base] = new;
            self.notify(id, old, new);
            self.record(id, new);
        }
        Ok(())
    }

    /// Sets one word of a memory (declared coordinates).
    pub fn set_word(&mut self, name: &str, index: i64, value: u128) -> Result<(), String> {
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
        let new = value & mask(width);
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

    /// Records every change of a visible signal (see [`Simulator::changes`]).
    pub fn set_recording(&mut self, on: bool) {
        self.recording = on;
    }

    /// Recorded value changes, in time order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// A VCD waveform of the recorded changes. Values are 2-state; every signal starts
    /// at zero at time 0.
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
        out.push_str("$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n");
        for sig in &vars {
            let s = &self.design.signals[*sig as usize];
            out.push_str(&vcd_value(0, s.width, &ids[sig]));
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

    fn record(&mut self, sig: SigId, value: u128) {
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
                    self.assign(lhs, v, *nb)?;
                    pc += 1;
                }
                Op::Jump(t) => pc = *t as usize,
                Op::JumpIf { cond, to } => {
                    pc = if self.eval(cond)? != 0 {
                        *to as usize
                    } else {
                        pc + 1
                    };
                }
                Op::JumpIfNot { cond, to } => {
                    pc = if self.eval(cond)? == 0 {
                        *to as usize
                    } else {
                        pc + 1
                    };
                }
                Op::Delay { amount, unit_ps } => {
                    let Some(_) = proc else {
                        return Err(self.err(format!("a delay inside the function of `{scope}`")));
                    };
                    let n = self.eval(amount)?;
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
    fn notify(&mut self, sig: SigId, old: u128, new: u128) {
        let list = std::mem::take(&mut self.waiters[sig as usize]);
        let mut keep = Vec::with_capacity(list.len());
        let mut fired = Vec::new();
        for w in list {
            let p = w.proc as usize;
            if !self.waiting[p] || self.epoch[p] != w.epoch {
                continue; // stale registration
            }
            let b = w.bit.unwrap_or(0);
            let (ob, nb) = ((old >> b) & 1, (new >> b) & 1);
            let fire = match w.edge {
                Edge::Any => match w.bit {
                    None => old != new,
                    Some(_) => ob != nb,
                },
                Edge::Pos => ob == 0 && nb == 1,
                Edge::Neg => ob == 1 && nb == 0,
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

    /// Stores `value` into `w` bits of word `idx` of `sig` at bit `lo`.
    fn write_bits(&mut self, sig: SigId, idx: usize, lo: u32, w: u32, value: u128) {
        let old = self.store[idx];
        let m = mask(w) << lo;
        let new = (old & !m) | ((value & mask(w)) << lo);
        if new != old {
            self.store[idx] = new;
            self.notify(sig, old, new);
            if self.design.signals[sig as usize].depth == 0 {
                self.record(sig, new);
            }
        }
    }

    fn assign(&mut self, lhs: &LVal, v: u128, nb: bool) -> Result<(), SimError> {
        let mut offset = lhs.w;
        for part in &lhs.parts {
            offset -= part.w;
            let pv = (v >> offset) & mask(part.w);
            let word = match &part.word {
                Some(ex) => {
                    let i = to_i64(self.eval(ex)?);
                    let pos = i - part.word_off;
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
                    let s = to_i64(self.eval(start)?);
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
        Ok(())
    }

    /// Evaluates an expression to its `w`-bit value.
    fn eval(&mut self, e: &Ex) -> Result<u128, SimError> {
        let w = e.w;
        Ok(match &e.k {
            K::Const(v) => v & mask(w),
            K::Sig(id) => {
                let s = &self.design.signals[*id as usize];
                self.store[s.base] & mask(w)
            }
            K::Word { sig, idx, off } => {
                let i = to_i64(self.eval(idx)?);
                let s = &self.design.signals[*sig as usize];
                let pos = i - off;
                if pos < 0 || pos >= s.depth as i64 {
                    0
                } else {
                    self.store[s.base + pos as usize] & mask(w)
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
                let s = to_i64(self.eval(start)?);
                let wi = w as i64;
                let a = if *up { s } else { s - (wi - 1) };
                let p = if *flip { off - (a + wi - 1) } else { a - off };
                let bw = base.w as i64;
                if p >= 0 && p + wi <= bw {
                    (bv >> p) & mask(w)
                } else {
                    let mut r = 0u128;
                    for k in 0..wi {
                        let q = p + k;
                        if q >= 0 && q < bw && (bv >> q) & 1 == 1 {
                            r |= 1u128 << k;
                        }
                    }
                    r
                }
            }
            K::Un(op, a) => {
                let x = self.eval(a)?;
                let aw = a.w;
                let m = mask(aw);
                match op {
                    UnOp::Plus => x,
                    UnOp::Minus => 0u128.wrapping_sub(x) & mask(w),
                    UnOp::BitNot => !x & mask(w),
                    UnOp::LNot => (x == 0) as u128,
                    UnOp::RedAnd => (x == m) as u128,
                    UnOp::RedNand => (x != m) as u128,
                    UnOp::RedOr => (x != 0) as u128,
                    UnOp::RedNor => (x == 0) as u128,
                    UnOp::RedXor => (x.count_ones() & 1) as u128,
                    UnOp::RedXnor => (1 ^ (x.count_ones() & 1)) as u128,
                }
            }
            K::Bin { op, a, b, os } => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                bin(*op, x, y, a.w, b.w, w, *os)
            }
            K::Tern(c, t, f) => {
                if self.eval(c)? != 0 {
                    self.eval(t)?
                } else {
                    self.eval(f)?
                }
            }
            K::Concat(parts) => {
                let mut acc = 0u128;
                for p in parts {
                    let v = self.eval(p)?;
                    acc = if p.w >= 128 { v } else { (acc << p.w) | v };
                }
                acc & mask(w)
            }
            K::Ext(a) => {
                let v = self.eval(a)?;
                if e.s && a.w > 0 && (v >> (a.w - 1)) & 1 == 1 {
                    (v | (mask(w) & !mask(a.w))) & mask(w)
                } else {
                    v & mask(w)
                }
            }
            K::Trunc(a) => self.eval(a)? & mask(w),
            K::Call { fid, args } => self.call(*fid as usize, args)?,
            K::Time(unit) => {
                let u = (*unit).max(1);
                (self.now / u) as u128 & mask(w)
            }
            K::MaskEq { a, b, mask: m } => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                ((x ^ y) & m == 0) as u128
            }
        })
    }

    fn call(&mut self, fid: usize, args: &[Ex]) -> Result<u128, SimError> {
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
            let new = v & mask(w);
            self.store[base] = new;
            if old != new {
                self.notify(*formal, old, new);
            }
        }
        match self.exec(&f.code, 0, None, &f.name)? {
            Exit::Halt | Exit::Wait(_) | Exit::Delay(_) | Exit::Finish(_) => {}
        }
        let r = &design.signals[f.ret as usize];
        Ok(self.store[r.base] & mask(r.width))
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
                ('c' | 'C', Some(Ok((v, _, _)))) => ((v as u8) as char).to_string(),
                ('s' | 'S', Some(Ok((v, w, _)))) => {
                    let mut s = String::new();
                    for k in (0..w.div_ceil(8)).rev() {
                        let b = ((v >> (k * 8)) & 0xFF) as u8;
                        if b != 0 {
                            s.push(b as char);
                        }
                    }
                    s
                }
                ('h' | 'H' | 'x' | 'X', Some(Ok((v, w, _)))) => {
                    let digits = w.div_ceil(4) as usize;
                    pad(&format!("{:0>digits$x}", v & mask(w)), width, zero, '0')
                }
                ('b' | 'B', Some(Ok((v, w, _)))) => {
                    let s = format!("{:0>w$b}", v & mask(w), w = w as usize);
                    pad(&s, width, zero, '0')
                }
                ('o' | 'O', Some(Ok((v, w, _)))) => {
                    let digits = w.div_ceil(3) as usize;
                    pad(&format!("{:0>digits$o}", v & mask(w)), width, zero, '0')
                }
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
            let radix = if hex { 16 } else { 2 };
            let mut v: u128 = 0;
            for ch in tok.chars().filter(|c| *c != '_') {
                let d = ch.to_digit(radix).unwrap_or(0) as u128;
                v = v.wrapping_mul(radix as u128).wrapping_add(d);
            }
            let pos = addr - s.mem_lo;
            if pos >= 0 && pos < s.depth as i64 {
                let idx = s.base + pos as usize;
                let old = self.store[idx];
                let new = v & mask(s.width);
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

const SNAP_MAGIC: &[u8; 8] = b"RTLENG01";

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
    /// Serialises the whole simulation state: values, process positions and waits,
    /// queues, timed events, the log. [`Simulator::restore`] reads it back into a
    /// simulator of the same design.
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
            put_u128(&mut out, *v);
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
            put_u128(&mut out, u.value);
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
            self.store[i] = r.u128()?;
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
            let value = r.u128()?;
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

    /// FNV-1a hash of the snapshot: equal states give equal hashes.
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.snapshot() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Binary operator on `w`-bit results with `aw`/`bw`-bit operands.
fn bin(op: BinOp, x: u128, y: u128, aw: u32, bw: u32, w: u32, os: bool) -> u128 {
    let m = mask(w);
    match op {
        BinOp::Add => x.wrapping_add(y) & m,
        BinOp::Sub => x.wrapping_sub(y) & m,
        BinOp::Mul => x.wrapping_mul(y) & m,
        BinOp::Div | BinOp::Mod => {
            if y == 0 {
                return 0; // 2-state: division by zero reads as 0
            }
            if os {
                let (a, b) = (sext(x, w), sext(y, w));
                if b == 0 {
                    return 0;
                }
                let r = if op == BinOp::Div {
                    a.wrapping_div(b)
                } else {
                    a.wrapping_rem(b)
                };
                (r as u128) & m
            } else if op == BinOp::Div {
                (x / y) & m
            } else {
                (x % y) & m
            }
        }
        BinOp::Pow => {
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
        BinOp::And => x & y & m,
        BinOp::Or => (x | y) & m,
        BinOp::Xor => (x ^ y) & m,
        BinOp::Xnor => !(x ^ y) & m,
        BinOp::Shl | BinOp::AShl => {
            if y >= 128 {
                0
            } else {
                (x << y) & m
            }
        }
        BinOp::Shr => {
            if y >= 128 {
                0
            } else {
                (x >> y) & m
            }
        }
        BinOp::AShr => {
            if os {
                let sh = y.min(127) as u32;
                ((sext(x, w) >> sh) as u128) & m
            } else if y >= 128 {
                0
            } else {
                (x >> y) & m
            }
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = if os {
                sext(x, aw).cmp(&sext(y, aw))
            } else {
                x.cmp(&y)
            };
            let r = match op {
                BinOp::Lt => ord.is_lt(),
                BinOp::Le => ord.is_le(),
                BinOp::Gt => ord.is_gt(),
                _ => ord.is_ge(),
            };
            let _ = bw;
            r as u128
        }
        BinOp::Eq | BinOp::CaseEq => (x == y) as u128,
        BinOp::Ne | BinOp::CaseNe => (x != y) as u128,
        BinOp::LAnd => (x != 0 && y != 0) as u128,
        BinOp::LOr => (x != 0 || y != 0) as u128,
    }
}

fn decimal(v: u128, w: u32, signed: bool) -> String {
    if signed {
        sext(v, w).to_string()
    } else {
        (v & mask(w)).to_string()
    }
}

/// Decimal digits needed for the largest `w`-bit value (plus a sign if signed).
fn max_decimal_digits(w: u32, signed: bool) -> usize {
    let max = if w >= 128 { u128::MAX } else { mask(w) };
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

fn vcd_value(v: u128, w: u32, id: &str) -> String {
    if w == 1 {
        format!("{}{}\n", v & 1, id)
    } else {
        let bits: String = (0..w)
            .rev()
            .map(|k| if (v >> k) & 1 == 1 { '1' } else { '0' })
            .collect();
        format!("b{bits} {id}\n")
    }
}
