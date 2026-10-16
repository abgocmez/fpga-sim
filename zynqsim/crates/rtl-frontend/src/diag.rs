//! Source positions, diagnostics and the step budget that bounds every pass.

use std::fmt;

/// A source position: file name (as given or as included), 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Pos {
    /// File name as it was opened (the include path for included files).
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column (byte offset within the line).
    pub col: u32,
}

impl Pos {
    /// Builds a position.
    pub fn new(file: &str, line: u32, col: u32) -> Pos {
        Pos {
            file: file.to_string(),
            line,
            col,
        }
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

/// One finding of the front end: a construct that is unsupported, or a syntax or
/// elaboration error. Printed as `file:line:col: construct: message`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Diagnostic {
    /// File of the construct.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// Short name of the construct, for example `typedef struct` or `syntax`.
    pub construct: String,
    /// What is wrong, in one sentence.
    pub message: String,
}

impl Diagnostic {
    /// Builds a diagnostic at `pos`.
    pub fn new(pos: &Pos, construct: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            file: pos.file.clone(),
            line: pos.line,
            column: pos.col,
            construct: construct.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {}",
            self.file, self.line, self.column, self.construct, self.message
        )
    }
}

/// A countdown of work units. Every pass takes one unit per token or step, so a
/// malformed input can never run forever: once the budget is spent the pass stops
/// and reports `budget exhausted`.
#[derive(Debug, Clone)]
pub struct Budget {
    remaining: u64,
    exhausted: bool,
}

impl Budget {
    /// A budget of `units` work units.
    pub fn new(units: u64) -> Budget {
        Budget {
            remaining: units,
            exhausted: false,
        }
    }

    /// Takes one unit. Returns `false` (and remembers it) when none is left.
    pub fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            return false;
        }
        self.remaining -= 1;
        true
    }

    /// Whether a `take` has ever failed.
    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}
