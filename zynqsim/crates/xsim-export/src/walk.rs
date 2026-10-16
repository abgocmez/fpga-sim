//! Finding the lists inside an export folder.

use std::path::{Path, PathBuf};

/// What a folder holds, each group sorted by name.
#[derive(Default)]
pub(crate) struct Found {
    pub scripts: Vec<PathBuf>,
    pub prjs: Vec<PathBuf>,
    pub lists: Vec<PathBuf>,
    pub opts: Vec<PathBuf>,
    pub inis: Vec<PathBuf>,
    /// More than one folder holds lists: the candidates.
    pub ambiguous: Vec<PathBuf>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
            && self.prjs.is_empty()
            && self.lists.is_empty()
            && self.opts.is_empty()
            && self.inis.is_empty()
    }
}

/// Directories that are products of a run, not part of an export.
const SKIP: &[&str] = &["xsim.dir", ".git", "node_modules", "ip_user_files", "target"];

/// How deep below the given folder lists are looked for (`export/xsim/`, `sim_1/synth/func/xsim`).
const MAX_DEPTH: usize = 4;

pub(crate) fn discover(dir: &Path) -> Found {
    let mut f = Found::default();
    walk(dir, 0, &mut f);
    // Several folders with lists (an export root holding `behav/xsim` and `postsynth`) are
    // different flows of one design: mixing them would compile a module twice. Report them and
    // let the caller choose one.
    let mut dirs: Vec<PathBuf> = f
        .scripts
        .iter()
        .chain(&f.prjs)
        .chain(&f.lists)
        .chain(&f.opts)
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs.dedup();
    if dirs.len() > 1 {
        f.ambiguous = dirs;
    }
    // Vivado names the Verilog list before the VHDL one (`*_vlog.prj`, `*_vhdl.prj`).
    f.prjs.sort_by_key(|p| {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        (p.parent().map(Path::to_path_buf), n.contains("vhdl"), n)
    });
    f
}

fn walk(dir: &Path, depth: usize, f: &mut Found) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if p.is_dir() {
            if depth < MAX_DEPTH && !SKIP.contains(&name.as_str()) {
                walk(&p, depth + 1, f);
            }
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "sh" | "bat" | "cmd" => f.scripts.push(p),
            "prj" => f.prjs.push(p),
            "f" => f.lists.push(p),
            "opt" => f.opts.push(p),
            "ini" if lower == "xsim.ini" => f.inis.push(p),
            _ => {}
        }
    }
}
