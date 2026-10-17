//! The flat, elaborated design the simulator runs.
//!
//! Hierarchy is gone: every net, variable and memory is a [`Signal`] with a
//! hierarchical name, every expression is an [`Ex`] whose width and signedness are
//! fixed at elaboration (Verilog's context-determined sizing is applied there), and
//! every behavioural statement is a flat list of [`Op`]s that the scheduler can
//! suspend and resume.

use rtl_frontend::ast::{BinOp, UnOp};

/// Index of a signal in [`Design::signals`].
pub type SigId = u32;

/// Largest width the engine represents in one value.
pub const MAX_WIDTH: u32 = 128;

/// A net, variable or memory of the flat design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    /// Hierarchical name (`top`-level names are plain, children are `inst.name`).
    pub name: String,
    /// Element width in bits (1 to 128).
    pub width: u32,
    /// Declared signed.
    pub signed: bool,
    /// Number of words of a memory; 0 for a scalar or vector.
    pub depth: u32,
    /// Index of the first word in the value store.
    pub base: usize,
    /// Lowest index of a memory (words are addressed `index - mem_lo`).
    pub mem_lo: i64,
    /// Declared `[msb:lsb]` with `msb < lsb`: bit `i` lives at position `off - i`.
    pub flip: bool,
    /// Declared `lsb` (or `msb` when flipped): the offset of the bit mapping.
    pub off: i64,
    /// Compiler-generated (locals of blocks, functions and tasks): not in name lookups.
    pub hidden: bool,
}

/// An expression with its width and signedness fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ex {
    /// Result width in bits.
    pub w: u32,
    /// Signedness of the node. Used for `Ext` (extension), `Bin` shifts and division,
    /// and `Ext`'s sign bit; the result of comparisons is unsigned.
    pub s: bool,
    /// The operation.
    pub k: K,
}

/// Expression kinds. Every kind produces exactly `w` bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum K {
    /// A constant (already masked to `w`).
    Const(u128),
    /// The whole value of a scalar or vector signal (`w` = its width).
    Sig(SigId),
    /// A memory word; `idx` maps to the word by `idx - off`. Out of range reads 0.
    Word {
        /// The memory.
        sig: SigId,
        /// The index expression (self-determined).
        idx: Box<Ex>,
        /// Lowest index of the memory.
        off: i64,
    },
    /// `w` bits of `base` starting at the position that the index range maps to.
    /// `start` is the index; `up` selects `[start +: w]` (or a single bit) and
    /// `!up` selects `[start -: w]`. Positions follow the declaration of the base.
    Slice {
        /// The vector (its full self-determined value).
        base: Box<Ex>,
        /// Start index (declared coordinates).
        start: Box<Ex>,
        /// Declared offset (lsb, or msb when `flip`).
        off: i64,
        /// Ascending declaration.
        flip: bool,
        /// `+:` (or a single bit).
        up: bool,
    },
    /// A unary operator on `a` (width `a.w`); reductions have `w` = 1.
    Un(UnOp, Box<Ex>),
    /// A binary operator. For comparisons the operands have the same width `a.w`
    /// and `os` gives their signedness; for logical operators `os` is unused.
    Bin {
        /// The operator.
        op: BinOp,
        /// Left operand.
        a: Box<Ex>,
        /// Right operand.
        b: Box<Ex>,
        /// Operand signedness (comparisons, `/`, `%`, `>>>`, `**`).
        os: bool,
    },
    /// `c ? t : f`.
    Tern(Box<Ex>, Box<Ex>, Box<Ex>),
    /// `{p0, p1, ...}` (MSB first).
    Concat(Vec<Ex>),
    /// Zero or sign extension of `a` to `w`, the sign taken from `s`.
    Ext(Box<Ex>),
    /// Truncation of `a` to `w`.
    Trunc(Box<Ex>),
    /// Call of function `fid`; `args` are assigned to its formals in order.
    Call {
        /// Function index.
        fid: u32,
        /// Actual arguments, already sized to the formals.
        args: Vec<Ex>,
    },
    /// `$time` in units of `unit_ps`.
    Time(u64),
    /// `(a ^ b) & mask == 0` (case-item match with don't-care bits); `w` = 1.
    MaskEq {
        /// Selector.
        a: Box<Ex>,
        /// Label.
        b: Box<Ex>,
        /// Bits that must match.
        mask: u128,
    },
}

/// Edge filter of a [`Trigger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Any change of the watched bit or value.
    Any,
    /// Rising edge (0 to 1) of the watched bit.
    Pos,
    /// Falling edge (1 to 0) of the watched bit.
    Neg,
}

/// One event a process waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    /// Watched signal.
    pub sig: SigId,
    /// Watched bit position (`None` for the whole value; edges use bit 0 then).
    pub bit: Option<u32>,
    /// Edge filter.
    pub edge: Edge,
}

/// A word/part of an assignment target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LPart {
    /// Target signal.
    pub sig: SigId,
    /// Memory word index (declared coordinates), if the target is a memory word.
    pub word: Option<Ex>,
    /// Declared offset of the word index (memories: `mem_lo`).
    pub word_off: i64,
    /// Part select: start index, declaration offset, flip, up. `None` for the whole
    /// (word or scalar) value.
    pub sel: Option<(Ex, i64, bool, bool)>,
    /// Width of the part.
    pub w: u32,
}

/// An assignment target: parts in MSB-first order, total width `w`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LVal {
    /// Total width.
    pub w: u32,
    /// Parts, most significant first.
    pub parts: Vec<LPart>,
}

/// Argument of a system task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// A string literal.
    Str(String),
    /// A value, with its signedness.
    Val(Ex, bool),
}

/// A system task executed by an [`Op::Sys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sys {
    /// `$display` (`newline`) or `$write`; the first argument is the format when it
    /// is a string.
    Display {
        /// Arguments.
        args: Vec<Arg>,
        /// Whether a newline ends the line.
        newline: bool,
        /// Hierarchical name for `%m`.
        scope: String,
    },
    /// `$finish` or `$stop`.
    Finish,
    /// `$readmemh` (`hex`) or `$readmemb` of a file into a memory.
    ReadMem {
        /// Memory.
        sig: SigId,
        /// File name as written (relative to the design's base directory).
        file: String,
        /// Hexadecimal words.
        hex: bool,
    },
}

/// One bytecode instruction of a process or function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `lhs = rhs` (blocking) or `lhs <= rhs` (non-blocking); `rhs.w` = `lhs.w`.
    Assign {
        /// Target.
        lhs: LVal,
        /// Value.
        rhs: Ex,
        /// Non-blocking.
        nb: bool,
    },
    /// Unconditional jump to an instruction index.
    Jump(u32),
    /// Jump if `cond` is non-zero.
    JumpIf {
        /// Condition.
        cond: Ex,
        /// Target.
        to: u32,
    },
    /// Jump if `cond` is zero.
    JumpIfNot {
        /// Condition.
        cond: Ex,
        /// Target.
        to: u32,
    },
    /// Suspend for `amount` time units of `unit_ps` picoseconds each.
    Delay {
        /// Amount.
        amount: Ex,
        /// Time unit in picoseconds.
        unit_ps: u64,
    },
    /// Suspend until one of the triggers fires (an event control or `wait`).
    Wait(Vec<Trigger>),
    /// A system task.
    Sys(Sys),
    /// End of a process (it does not run again) or of a function body.
    Halt,
}

/// What a process is, for diagnostics and the start-up order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcKind {
    /// Declaration initialisers of variables (run once, first).
    Init,
    /// `initial`.
    Initial,
    /// `always` (any form) and `always_ff`.
    Always,
    /// `always_comb` and `always_latch`: they start at time 0 with their sensitivity
    /// registered, as a continuous assignment does.
    Comb,
    /// Continuous assignment (`assign`, gates, port connections, `wire` initialisers).
    /// Code starts with a `Wait` on its inputs; the simulator starts it at time 0
    /// with that wait already registered.
    Assign,
}

/// A process of the flat design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// Kind.
    pub kind: ProcKind,
    /// Hierarchical name of the scope (for `%m` and messages).
    pub scope: String,
    /// Bytecode.
    pub code: Vec<Op>,
}

/// A function: its body runs to completion without suspending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Func {
    /// Name, for messages.
    pub name: String,
    /// Formal arguments (signals), in order.
    pub formals: Vec<SigId>,
    /// Return variable.
    pub ret: SigId,
    /// Bytecode, ending in `Halt`.
    pub code: Vec<Op>,
}
