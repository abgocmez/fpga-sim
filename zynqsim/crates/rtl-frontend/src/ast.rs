//! Abstract syntax tree of the native front end.
//!
//! The tree keeps what the elaborator and a later engine need: declarations with
//! their ranges, every assignment and every process, with positions. Constructs
//! that cannot be represented are not in the tree; the parser reports them as
//! [`crate::Diagnostic`]s instead.

use crate::diag::Pos;
use crate::lexer::Number;

/// One parsed source file.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SourceUnit {
    /// File name.
    pub file: String,
    /// Top-level items in source order.
    pub items: Vec<Item>,
    /// Argument of the last `` `timescale `` in the file.
    pub timescale: Option<String>,
    /// Argument of the last `` `default_nettype `` in the file.
    pub default_nettype: Option<String>,
}

/// A top-level item.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Item {
    /// A module.
    Module(Module),
    /// A package-level `typedef`.
    Typedef(Typedef),
}

/// A module declaration.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Module {
    /// Module name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// Parameters of the `#( ... )` header, in order.
    pub params: Vec<ParamDecl>,
    /// Ports, in header order (non-ANSI names are completed by body declarations).
    pub ports: Vec<PortDecl>,
    /// Whether the header used ANSI-style port declarations.
    pub ansi: bool,
    /// Body items in source order.
    pub items: Vec<ModuleItem>,
}

/// Port direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Dir {
    /// `input`.
    Input,
    /// `output`.
    Output,
    /// `inout`.
    Inout,
}

impl Dir {
    /// The keyword spelling of the direction.
    pub fn keyword(self) -> &'static str {
        match self {
            Dir::Input => "input",
            Dir::Output => "output",
            Dir::Inout => "inout",
        }
    }
}

/// Kind of a declared object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum DataKind {
    /// `wire` (also `tri`, `wand`, `wor`, `supply0`, `supply1` are mapped here).
    Wire,
    /// `reg`.
    Reg,
    /// `logic` (SystemVerilog 4-state variable).
    Logic,
    /// `bit` (SystemVerilog 2-state variable).
    Bit,
    /// `integer` (32-bit signed variable).
    Integer,
    /// `int` (32-bit signed 2-state variable).
    Int,
    /// `time` (64-bit unsigned variable).
    Time,
    /// `genvar`.
    Genvar,
    /// A type name declared by a `typedef` of this file.
    Named(String),
}

/// A packed or unpacked range `[msb:lsb]`. A SystemVerilog `[n]` dimension is stored
/// as `[n-1:0]`-style with `lsb` = 0 and `msb` = the expression `n` minus one.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Range {
    /// Most significant bound.
    pub msb: Expr,
    /// Least significant bound.
    pub lsb: Expr,
}

/// A port, from the header or from a body direction declaration.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PortDecl {
    /// Port name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// Direction; `None` when a non-ANSI port has no direction declaration.
    pub dir: Option<Dir>,
    /// Data kind; `None` means the default (`wire`).
    pub kind: Option<DataKind>,
    /// `signed` was given.
    pub signed: bool,
    /// Packed range, if any.
    pub range: Option<Range>,
}

/// A parameter or localparam.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ParamDecl {
    /// Parameter name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// `localparam` (cannot be overridden).
    pub local: bool,
    /// `signed` was given.
    pub signed: bool,
    /// Packed range, if any.
    pub range: Option<Range>,
    /// Default value.
    pub value: Option<Expr>,
}

/// A variable or net declaration.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Decl {
    /// Declared name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// Data kind.
    pub kind: DataKind,
    /// `signed` was given.
    pub signed: bool,
    /// Packed range, if any.
    pub range: Option<Range>,
    /// Unpacked dimensions (memories): `reg [7:0] mem [0:15]`.
    pub dims: Vec<Range>,
    /// Declaration assignment (`wire x = a & b;` or `reg r = 0;`).
    pub init: Option<Expr>,
}

/// A `typedef`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Typedef {
    /// Type name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// What the name denotes.
    pub kind: TypedefKind,
}

/// The right-hand side of a `typedef`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum TypedefKind {
    /// A vector or scalar type (`typedef logic [7:0] byte_t;`).
    Vector {
        /// Packed range, if any.
        range: Option<Range>,
        /// `signed` was given.
        signed: bool,
    },
    /// An enumeration; the member names become constants.
    Enum(Vec<String>),
}

/// Something that can appear in a module body.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ModuleItem {
    /// `parameter` or `localparam` in the body.
    Param(ParamDecl),
    /// Net, variable or genvar declaration.
    Decl(Decl),
    /// `typedef` in the body.
    Typedef(Typedef),
    /// Continuous assignment (one per left-hand side).
    Assign(ContAssign),
    /// `always`, `always_ff`, `always_comb`, `always_latch`.
    Always(Always),
    /// `initial`. Kept for testbenches; not synthesizable.
    Initial(Initial),
    /// Module or gate-primitive instantiation.
    Instance(Instance),
    /// `task` or `function`.
    Routine(Routine),
    /// A generate construct or a `generate` region.
    Generate(Generate),
    /// `import pkg::name;` or `import pkg::*;` (names are not resolved here).
    Import(Import),
}

/// A package import inside a module.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Import {
    /// Package name.
    pub package: String,
    /// Imported name, or `None` for `*`.
    pub item: Option<String>,
    /// Position.
    pub pos: Pos,
}

impl ModuleItem {
    /// Whether the item is part of the synthesizable subset (`initial` is not).
    pub fn is_synthesizable(&self) -> bool {
        !matches!(self, ModuleItem::Initial(_))
    }
}

/// `assign lhs = rhs;`
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ContAssign {
    /// Driven net.
    pub lhs: Expr,
    /// Driving expression.
    pub rhs: Expr,
    /// `#delay` after `assign`, if any (parsed, not synthesizable).
    pub delay: Option<Expr>,
    /// Position of `assign`.
    pub pos: Pos,
}

/// Kind of an always block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum AlwaysKind {
    /// Plain `always`.
    Plain,
    /// `always_ff`.
    Ff,
    /// `always_comb`.
    Comb,
    /// `always_latch`.
    Latch,
}

/// An always block.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Always {
    /// Which keyword introduced it.
    pub kind: AlwaysKind,
    /// The body; for `always @(...)` it starts with an [`StmtKind::EventCtrl`].
    pub body: Stmt,
    /// Position of the keyword.
    pub pos: Pos,
}

/// An initial block (behavioural; flagged as not synthesizable).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Initial {
    /// The body.
    pub body: Stmt,
    /// Position of the keyword.
    pub pos: Pos,
}

/// A module instance or a gate primitive.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Instance {
    /// Module (or primitive keyword) name.
    pub module: String,
    /// Instance name (empty for an unnamed primitive).
    pub name: String,
    /// Position of the instance name.
    pub pos: Pos,
    /// Parameter overrides (`#(...)`).
    pub params: Vec<Conn>,
    /// Port connections.
    pub ports: Vec<Conn>,
    /// `.*` implicit connections were used.
    pub wildcard: bool,
    /// Instance array dimension `name [N]`, if any.
    pub array: Option<Range>,
    /// The instance is a gate primitive (`and`, `buf`, ...).
    pub primitive: bool,
}

/// A parameter override or a port connection.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Conn {
    /// Formal name for a named connection, `None` for a positional one.
    pub name: Option<String>,
    /// Actual expression; `None` for an empty connection `.a()`.
    pub expr: Option<Expr>,
    /// Position.
    pub pos: Pos,
}

/// Whether a routine is a task or a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum RoutineKind {
    /// `task`.
    Task,
    /// `function`.
    Function,
}

/// A task or function.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Routine {
    /// Task or function.
    pub kind: RoutineKind,
    /// Name.
    pub name: String,
    /// Position of the name.
    pub pos: Pos,
    /// `automatic` (otherwise static).
    pub automatic: bool,
    /// Return data kind (functions only); `None` means 1-bit.
    pub ret_kind: Option<DataKind>,
    /// Return range (functions only).
    pub ret_range: Option<Range>,
    /// Return type is signed.
    pub ret_signed: bool,
    /// Formal arguments (direction defaults to input).
    pub args: Vec<PortDecl>,
    /// Local declarations.
    pub decls: Vec<Decl>,
    /// Body statements.
    pub body: Vec<Stmt>,
}

/// A generate construct.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Generate {
    /// Block label, if any.
    pub label: Option<String>,
    /// Position.
    pub pos: Pos,
    /// What it is.
    pub kind: GenKind,
}

/// The shape of a generate construct.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum GenKind {
    /// `begin ... end` (or a bare `generate` region).
    Block(Vec<ModuleItem>),
    /// `for (genvar = init; cond; var = step) begin ... end`.
    For {
        /// Loop variable.
        var: String,
        /// Initial value.
        init: Expr,
        /// Continuation condition.
        cond: Expr,
        /// Step: the new value of `var`, written in terms of `var`.
        step: Expr,
        /// Body.
        body: Vec<ModuleItem>,
    },
    /// `if (cond) ... else ...`.
    If {
        /// Condition (must be constant).
        cond: Expr,
        /// Taken when the condition is true.
        then_items: Vec<ModuleItem>,
        /// Taken otherwise, if present.
        else_items: Option<Vec<ModuleItem>>,
    },
    /// `case (expr) ... endcase`.
    Case {
        /// Selector (must be constant).
        expr: Expr,
        /// Items; an empty label list is the `default` item.
        items: Vec<(Vec<Expr>, Vec<ModuleItem>)>,
    },
}

/// Edge of an event expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Edge {
    /// No edge keyword.
    Any,
    /// `posedge`.
    Pos,
    /// `negedge`.
    Neg,
}

/// One event of an event control.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Event {
    /// Edge keyword.
    pub edge: Edge,
    /// Watched signal.
    pub expr: Expr,
}

/// Which case statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum CaseKind {
    /// `case`.
    Case,
    /// `casez`.
    Casez,
    /// `casex`.
    Casex,
}

/// One item of a case statement.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CaseItem {
    /// Labels; empty for `default`.
    pub labels: Vec<Expr>,
    /// Statement executed for this item.
    pub body: Stmt,
}

/// A statement with its position.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Stmt {
    /// Position of the first token.
    pub pos: Pos,
    /// The statement.
    pub kind: StmtKind,
}

/// Statement forms.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum StmtKind {
    /// `;`
    Null,
    /// `begin [: name] decls stmts end`.
    Block {
        /// Block name, if any.
        name: Option<String>,
        /// Declarations local to the block.
        decls: Vec<Decl>,
        /// Statements.
        stmts: Vec<Stmt>,
    },
    /// `lhs = rhs;`, `lhs <= rhs;`, and `lhs op= rhs;` / `lhs++;` (rewritten).
    Assign {
        /// Target.
        lhs: Expr,
        /// Value.
        rhs: Expr,
        /// Non-blocking (`<=`).
        nonblocking: bool,
        /// Intra-assignment or leading `#delay` (parsed, not synthesizable).
        delay: Option<Expr>,
    },
    /// `if (cond) then [else otherwise]`.
    If {
        /// Condition.
        cond: Expr,
        /// Then branch.
        then_stmt: Box<Stmt>,
        /// Else branch.
        else_stmt: Option<Box<Stmt>>,
    },
    /// `case`, `casez`, `casex`.
    Case {
        /// Which case.
        kind: CaseKind,
        /// Selector.
        expr: Expr,
        /// Items.
        items: Vec<CaseItem>,
    },
    /// `for (init; cond; step) body`.
    For {
        /// Initialisation (an assignment, or a declaration with initialiser).
        init: Box<Stmt>,
        /// Condition.
        cond: Expr,
        /// Step (an assignment).
        step: Box<Stmt>,
        /// Body.
        body: Box<Stmt>,
    },
    /// `while (cond) body`.
    While {
        /// Condition.
        cond: Expr,
        /// Body.
        body: Box<Stmt>,
    },
    /// `repeat (count) body`.
    Repeat {
        /// Count.
        count: Expr,
        /// Body.
        body: Box<Stmt>,
    },
    /// `forever body`.
    Forever {
        /// Body.
        body: Box<Stmt>,
    },
    /// `#amount stmt` (parsed, not synthesizable).
    Delay {
        /// Delay amount.
        amount: Expr,
        /// Delayed statement.
        stmt: Box<Stmt>,
    },
    /// `@(events) stmt`, or `@*` (`events` is `None`).
    EventCtrl {
        /// The events; `None` for `@*`.
        events: Option<Vec<Event>>,
        /// Guarded statement.
        stmt: Box<Stmt>,
    },
    /// `wait (cond) stmt` (parsed, not synthesizable).
    Wait {
        /// Condition.
        cond: Expr,
        /// Guarded statement.
        stmt: Box<Stmt>,
    },
    /// `$name(args);`
    SysCall {
        /// System task name including `$`.
        name: String,
        /// Arguments.
        args: Vec<Expr>,
    },
    /// Call of a user task: `name(args);`.
    Call {
        /// Task name.
        name: String,
        /// Arguments.
        args: Vec<Expr>,
    },
    /// `return [expr];`
    Return(Option<Expr>),
    /// `break;`
    Break,
    /// `continue;`
    Continue,
    /// `disable name;` (parsed, not synthesizable).
    Disable(String),
}

/// An expression with its position.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Expr {
    /// Position of the first token.
    pub pos: Pos,
    /// The expression.
    pub kind: ExprKind,
}

/// Expression forms.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ExprKind {
    /// Identifier, or a dotted hierarchical name such as `u_core.pc`.
    Ident(String),
    /// Integer literal.
    Number(Number),
    /// String literal.
    Str(String),
    /// Unary operator.
    Unary(UnOp, Box<Expr>),
    /// Binary operator.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `c ? t : f`.
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `{a, b, ...}`.
    Concat(Vec<Expr>),
    /// `{n{a, b}}`.
    Repl(Box<Expr>, Vec<Expr>),
    /// `base[index]`.
    Index(Box<Expr>, Box<Expr>),
    /// `base[msb:lsb]`.
    Part(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `base[start +: width]` (`up` true) or `base[start -: width]`.
    IndexedPart {
        /// Selected vector.
        base: Box<Expr>,
        /// Start index.
        start: Box<Expr>,
        /// Width.
        width: Box<Expr>,
        /// `+:` (true) or `-:` (false).
        up: bool,
    },
    /// Call of a user function.
    Call(String, Vec<Expr>),
    /// `$name(args)` used as a value.
    SysCall(String, Vec<Expr>),
    /// `N'(expr)` size cast (`size` is `None` for `'(expr)`).
    Cast(Option<u32>, Box<Expr>),
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum UnOp {
    /// `+`.
    Plus,
    /// `-`.
    Minus,
    /// `!`.
    LNot,
    /// `~`.
    BitNot,
    /// `&` (reduction).
    RedAnd,
    /// `~&`.
    RedNand,
    /// `|` (reduction).
    RedOr,
    /// `~|`.
    RedNor,
    /// `^` (reduction).
    RedXor,
    /// `~^` / `^~` (reduction).
    RedXnor,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum BinOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `%`.
    Mod,
    /// `**`.
    Pow,
    /// `<<`.
    Shl,
    /// `>>`.
    Shr,
    /// `<<<`.
    AShl,
    /// `>>>`.
    AShr,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `===`.
    CaseEq,
    /// `!==`.
    CaseNe,
    /// `&`.
    And,
    /// `|`.
    Or,
    /// `^`.
    Xor,
    /// `~^` / `^~`.
    Xnor,
    /// `&&`.
    LAnd,
    /// `||`.
    LOr,
}
