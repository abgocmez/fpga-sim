//! The elaborated design and its entry points.
//!
//! [`Design::elaborate`] reads sources with `rtl-frontend`, flattens the hierarchy from
//! the top module, and lowers every process into the [`crate::ir`] form. Problems that
//! the engine cannot represent are collected in [`Design::diagnostics`]; a design with
//! diagnostics is refused by [`crate::Simulator::new`].

use crate::elab::Elab;
use crate::ir::{Func, Proc, SigId, Signal};
use rtl_frontend::ast::Item;
use rtl_frontend::parser::{Options, Parsed, parse_files};
use rtl_frontend::{Diagnostic, Pos};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A flat, elaborated design.
#[derive(Debug, Clone)]
pub struct Design {
    /// Name of the top module.
    pub top: String,
    /// Every signal (named, hidden locals and memories).
    pub signals: Vec<Signal>,
    /// Length of the value store in words.
    pub store_len: usize,
    /// Processes: declaration initialisers first, then the rest in source order.
    pub procs: Vec<Proc>,
    /// Functions.
    pub funcs: Vec<Func>,
    /// Visible (non-hidden) names to signal ids.
    pub names: HashMap<String, SigId>,
    /// Directory that relative `$readmem` file names are resolved against; the
    /// current directory when `None`.
    pub base_dir: Option<PathBuf>,
    /// Problems found while elaborating. A design with any is refused by the simulator.
    pub diagnostics: Vec<Diagnostic>,
}

impl Design {
    /// Elaborates `top` from source files, with `NAME[=VALUE]` defines and include
    /// directories as for `rtl-frontend`.
    pub fn elaborate(
        top: &str,
        sources: &[PathBuf],
        defines: &[String],
        include_dirs: &[PathBuf],
    ) -> Design {
        let opts = Options {
            defines: defines.to_vec(),
            include_dirs: include_dirs.to_vec(),
            ..Options::default()
        };
        let parsed = parse_files(sources, &opts);
        Design::elaborate_parsed(top, &parsed)
    }

    /// Elaborates `top` from already parsed units.
    pub fn elaborate_parsed(top: &str, parsed: &Parsed) -> Design {
        let mut diags: Vec<Diagnostic> = parsed.diagnostics.clone();
        // Timescale of each module's file (the last `timescale` in the file).
        let mut units: HashMap<String, (u64, String)> = HashMap::new();
        for unit in &parsed.units {
            let ps = unit.timescale.as_deref().map(timescale_ps).unwrap_or(1000);
            for item in &unit.items {
                if let Item::Module(m) = item {
                    units
                        .entry(m.name.clone())
                        .or_insert((ps, unit.file.clone()));
                }
            }
        }
        let mut el = Elab::new(parsed, units);
        let hier = el.top(top);
        diags.append(&mut el.diags);
        let mut design = Design {
            top: top.to_string(),
            signals: el.signals,
            store_len: el.store_len,
            procs: [el.init_procs, el.procs].concat(),
            funcs: el.funcs,
            names: HashMap::new(),
            base_dir: None,
            diagnostics: diags,
        };
        if !hier {
            design.diagnostics.push(Diagnostic::new(
                &Pos::new("<elaboration>", 0, 0),
                "elaboration",
                format!("top module `{top}` is not defined"),
            ));
        }
        for (i, s) in design.signals.iter().enumerate() {
            if !s.hidden {
                design.names.insert(s.name.clone(), i as SigId);
            }
        }
        design
    }

    /// The signal with this hierarchical name, if visible.
    pub fn signal(&self, name: &str) -> Option<SigId> {
        self.names.get(name).copied()
    }

    /// Sets the directory that relative `$readmem` file names are resolved against.
    pub fn with_base_dir(mut self, dir: &Path) -> Design {
        self.base_dir = Some(dir.to_path_buf());
        self
    }
}

/// Picoseconds in one unit of a `` `timescale `` argument such as `1ns / 1ps`.
/// Units finer than a picosecond are rounded to one picosecond.
pub fn timescale_ps(arg: &str) -> u64 {
    let unit = arg.split('/').next().unwrap_or("").trim();
    let digits: String = unit.chars().take_while(|c| c.is_ascii_digit()).collect();
    let letters: String = unit.chars().skip(digits.len()).collect();
    let n: u64 = digits.parse().unwrap_or(1);
    let per = match letters.trim() {
        "s" => 1_000_000_000_000,
        "ms" => 1_000_000_000,
        "us" => 1_000_000,
        "ns" => 1_000,
        _ => 1,
    };
    (n * per).max(1)
}

#[cfg(test)]
mod tests {
    use super::timescale_ps;

    #[test]
    fn timescale_units_convert_to_picoseconds() {
        assert_eq!(timescale_ps("1ns / 1ps"), 1000);
        assert_eq!(timescale_ps("10 ps"), 10);
        assert_eq!(timescale_ps("1us/1ns"), 1_000_000);
        assert_eq!(timescale_ps("1fs"), 1);
    }
}
