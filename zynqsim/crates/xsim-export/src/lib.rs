//! Reads Vivado simulation exports into an ordered description of what to compile.
//!
//! Vivado's `export_simulation -simulator xsim` (and `launch_simulation -scripts_only`) write a
//! folder holding `xvlog`/`xvhdl` project lists (`*.prj`: `verilog LIB "file"`, `sv LIB ...`,
//! `vhdl LIB ...` with `-i`, `-d`, `-L` options), sometimes plain `.f` lists, an `elab.opt`
//! with the `xelab` options and top units, an `xsim.ini` with library mappings, and
//! shell/batch scripts that tie them together. This crate reads those into an
//! [`ExportDesign`]: source files in compile order with their language and library, include
//! directories, defines, the `-L` libraries, the elaboration tops and library mappings.
//!
//! It is a *reader*: it runs no tool and has no dependencies. Whatever it does not understand
//! is reported as a [`Diagnostic`] with file, line and the offending text, never skipped
//! silently (prompt section 9.1). Paths in a list are relative to the list's directory (the
//! scripts `cd` there before calling the tools).

mod lex;
mod parse;
mod walk;

use std::fmt;
use std::path::{Path, PathBuf};

pub use parse::Reader;

/// Source language of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    /// Verilog-2001 (`xvlog`).
    Verilog,
    /// SystemVerilog (`xvlog -sv`).
    SystemVerilog,
    /// VHDL-93 (`xvhdl`).
    Vhdl,
    /// VHDL-2008 (`xvhdl --2008`).
    Vhdl2008,
}

impl Lang {
    /// The keyword a `.prj` line starts with.
    pub fn prj_word(self) -> &'static str {
        match self {
            Lang::Verilog => "verilog",
            Lang::SystemVerilog => "sv",
            Lang::Vhdl => "vhdl",
            Lang::Vhdl2008 => "vhdl2008",
        }
    }

    /// `true` for the VHDL variants.
    pub fn is_vhdl(self) -> bool {
        matches!(self, Lang::Vhdl | Lang::Vhdl2008)
    }

    /// The language a file extension implies (`.v`, `.sv`, `.vhd`, `.vhdl`).
    pub fn from_path(p: &Path) -> Option<Lang> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "v" | "vp" => Some(Lang::Verilog),
            "sv" | "svp" => Some(Lang::SystemVerilog),
            "vhd" | "vhdl" => Some(Lang::Vhdl),
            _ => None,
        }
    }
}

/// Where in an input list a source came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The list or script.
    pub file: PathBuf,
    /// 1-based line.
    pub line: usize,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

/// One file to compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    /// Resolved path (absolute, or relative to where the lists were read from).
    pub path: PathBuf,
    /// Language.
    pub lang: Lang,
    /// Library it compiles into (`xil_defaultlib`, `xpm`, ...).
    pub library: String,
    /// Include directories given on its own `.prj` line, resolved.
    pub includes: Vec<PathBuf>,
    /// Defines given on its own line (`NAME` or `NAME=VALUE`).
    pub defines: Vec<String>,
    /// The list line that named it.
    pub origin: Origin,
}

/// How serious a [`Diagnostic`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Ignored; results may differ from the vendor flow.
    Warning,
    /// The design cannot be built as described.
    Error,
}

/// Something in the export that is not supported or not usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// The list or script.
    pub file: PathBuf,
    /// 1-based line (0 when the whole file is meant).
    pub line: usize,
    /// The offending text (an option, a path, a keyword).
    pub construct: String,
    /// Why, and what to do.
    pub reason: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: `{}`: {}",
            self.file.display(),
            self.line,
            self.construct,
            self.reason
        )
    }
}

/// Everything read from an export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportDesign {
    /// Sources in compile order (duplicates removed, first wins).
    pub sources: Vec<SourceFile>,
    /// Include directories that apply to every file (`+incdir+`, script `-i`).
    pub include_dirs: Vec<PathBuf>,
    /// Defines that apply to every file (`NAME` or `NAME=VALUE`).
    pub defines: Vec<String>,
    /// Libraries searched at elaboration (`-L`): `unisims_ver`, `xpm`, `secureip`, ...
    pub link_libs: Vec<String>,
    /// Library name to directory mappings (`xsim.ini`, `-L name=dir`).
    pub lib_maps: Vec<(String, PathBuf)>,
    /// Elaboration top units as written (`xil_defaultlib.tb`, `xil_defaultlib.glbl`).
    pub tops: Vec<String>,
    /// Top-level parameter overrides (`-generic_top NAME=VALUE`).
    pub generics: Vec<String>,
    /// `--relax` was given to a compiler or the elaborator.
    pub relax: bool,
    /// `-timescale` of the elaboration, if given.
    pub timescale: Option<String>,
    /// Every list/script/option file that was read, in order.
    pub inputs: Vec<PathBuf>,
    /// `glbl.v` was named but is not a file on disk (typically `$XILINX_VIVADO/data/verilog/src`);
    /// the consumer supplies Vivado's own copy.
    pub glbl_missing: bool,
    /// Everything not supported or not usable.
    pub diagnostics: Vec<Diagnostic>,
}

impl ExportDesign {
    /// The distinct libraries of the sources, in first-use order.
    pub fn libraries(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.sources {
            if !out.contains(&s.library) {
                out.push(s.library.clone());
            }
        }
        out
    }

    /// The library files without a library of their own belong to: the first source's, else
    /// `xil_defaultlib` (Vivado's default).
    pub fn default_library(&self) -> String {
        self.sources
            .first()
            .map(|s| s.library.clone())
            .unwrap_or_else(|| "xil_defaultlib".to_string())
    }

    /// `true` when some diagnostic is an [`Severity::Error`].
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// A source file is `glbl.v` (the UNISIM global-signal module).
    pub fn has_glbl_source(&self) -> bool {
        self.sources.iter().any(|s| {
            s.path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("glbl.v"))
        })
    }

    /// Top units that are not `glbl`, as bare module names (`xil_defaultlib.tb` -> `tb`).
    pub fn top_modules(&self) -> Vec<String> {
        self.tops
            .iter()
            .map(|t| t.rsplit('.').next().unwrap_or(t).to_string())
            .filter(|m| !m.eq_ignore_ascii_case("glbl"))
            .collect()
    }

    /// The defines for one source: global ones, then its own.
    pub fn defines_for(&self, s: &SourceFile) -> Vec<String> {
        self.defines
            .iter()
            .chain(s.defines.iter())
            .cloned()
            .collect()
    }

    /// The include directories for one source: its own, then the global ones.
    pub fn includes_for(&self, s: &SourceFile) -> Vec<PathBuf> {
        s.includes
            .iter()
            .chain(self.include_dirs.iter())
            .cloned()
            .collect()
    }
}

/// Reads an export: a folder, a `.prj`, a `.f`, an `elab.opt`, an `xsim.ini` or a script.
/// Variables such as `$XILINX_VIVADO` come from the process environment.
pub fn read(path: &Path) -> Result<ExportDesign, String> {
    Reader::new().read(path)
}

#[cfg(test)]
mod tests;
