//! Finding Verilator and the programs its build runs.
//!
//! Order for the Verilator driver (`verilator_bin.exe`, the C++ binary):
//! `ZYNQSIM_VERILATOR` (a file, the `verilator` launcher next to it, or a directory), then
//! `VERILATOR_ROOT`, then `PATH`, then the default unpacked-archive folder
//! `<temp>/zs-tools/verilator/oss-cad-suite/bin` (see `docs/INSTALL.md`).
//!
//! The launcher `verilator` is a Perl script; it is used only when no binary is found.

use std::path::{Path, PathBuf};

/// What was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerilatorTools {
    /// The driver executable.
    pub verilator: PathBuf,
    /// `VERILATOR_ROOT` (the `share/verilator` directory), when it can be found.
    pub root: Option<PathBuf>,
    /// Directories put first on `PATH` for the build (Verilator's own `bin` and `lib`, the
    /// folders of `make` and of a POSIX `sh` when they are not on `PATH`).
    pub tool_dirs: Vec<PathBuf>,
}

/// The default unpacked-archive folder, relative to the temporary directory.
pub const DEFAULT_TOOLS_DIR: &[&str] = &["zs-tools", "verilator", "oss-cad-suite"];

fn bin_names() -> [&'static str; 2] {
    if cfg!(windows) {
        ["verilator_bin.exe", "verilator.exe"]
    } else {
        ["verilator_bin", "verilator"]
    }
}

/// `dir/name` when it is a file.
fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// The driver inside a directory, preferring the binary to the launcher.
fn driver_in(dir: &Path) -> Option<PathBuf> {
    bin_names().iter().find_map(|n| in_dir(dir, n))
}

/// Accepts a file (the binary, or the launcher with the binary beside it) or a directory
/// that contains a driver (or a `bin` folder that does).
fn from_explicit(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        let dir = p.parent()?;
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.starts_with("verilator_bin") {
            return Some(p.to_path_buf());
        }
        return driver_in(dir).or_else(|| Some(p.to_path_buf()));
    }
    if p.is_dir() {
        return driver_in(p).or_else(|| driver_in(&p.join("bin")));
    }
    None
}

/// The directory of `VERILATOR_ROOT` for a driver in `bin`: `<prefix>/share/verilator`.
fn root_for(bin_dir: &Path) -> Option<PathBuf> {
    let cand = bin_dir.parent()?.join("share").join("verilator");
    cand.join("include").is_dir().then_some(cand)
}

/// The first directory on `PATH` that holds `name` (with `.exe` on Windows).
fn on_path(name: &str) -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(format!("{name}{suffix}")))
        .find(|p| p.is_file())
}

/// The default folder of the unpacked oss-cad-suite archive, if it exists.
fn default_suite() -> Option<PathBuf> {
    let mut p = std::env::temp_dir();
    for part in DEFAULT_TOOLS_DIR {
        p.push(part);
    }
    p.is_dir().then_some(p)
}

/// Finds Verilator. `None` when no driver is found anywhere in the search order.
pub fn find() -> Option<VerilatorTools> {
    let verilator = std::env::var_os("ZYNQSIM_VERILATOR")
        .and_then(|v| from_explicit(Path::new(&v)))
        .or_else(|| {
            let root = std::env::var_os("VERILATOR_ROOT")?;
            // `<prefix>/share/verilator` -> `<prefix>/bin`.
            let bin = Path::new(&root).parent()?.parent()?.join("bin");
            driver_in(&bin)
        })
        .or_else(|| on_path("verilator_bin").or_else(|| on_path("verilator")))
        .or_else(|| driver_in(&default_suite()?.join("bin")))?;
    let bin_dir = verilator.parent()?.to_path_buf();
    let root = std::env::var_os("VERILATOR_ROOT")
        .map(PathBuf::from)
        .filter(|p| p.join("include").is_dir())
        .or_else(|| root_for(&bin_dir));
    let mut tool_dirs = vec![bin_dir.clone()];
    if let Some(lib) = bin_dir
        .parent()
        .map(|p| p.join("lib"))
        .filter(|p| p.is_dir())
    {
        // The oss-cad-suite keeps its `python3` there.
        tool_dirs.push(lib);
    }
    // `make` and `sh` outside PATH: the copy made for this machine and Git for Windows.
    if on_path("make").is_none() {
        if let Some(dir) = default_tools_dir().filter(|d| d.join("make.exe").is_file()) {
            tool_dirs.push(dir);
        }
    }
    if on_path("sh").is_none() {
        let git_sh = PathBuf::from(r"C:\Program Files\Git\usr\bin");
        if git_sh.join("sh.exe").is_file() {
            tool_dirs.push(git_sh);
        }
    }
    Some(VerilatorTools {
        verilator,
        root,
        tool_dirs,
    })
}

/// `<temp>/zs-tools/tools`: where a copy of GNU make can be placed (`make.exe`).
pub fn default_tools_dir() -> Option<PathBuf> {
    let mut p = std::env::temp_dir();
    p.push("zs-tools");
    p.push("tools");
    p.is_dir().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_locations_resolve_to_the_binary() {
        // An empty folder is not a driver location.
        let dir = std::env::temp_dir().join(format!("zs-verilator-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        assert_eq!(from_explicit(&dir), None);
        // A launcher with the binary beside it resolves to the binary.
        std::fs::write(dir.join("bin").join("verilator"), b"#!/usr/bin/perl\n").unwrap();
        std::fs::write(dir.join("bin").join(bin_names()[0]), b"MZ").unwrap();
        assert_eq!(
            from_explicit(&dir.join("bin").join("verilator")),
            Some(dir.join("bin").join(bin_names()[0]))
        );
        // A directory holding a `bin` folder resolves too.
        assert_eq!(
            from_explicit(&dir),
            Some(dir.join("bin").join(bin_names()[0]))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_share_root_is_found_next_to_the_bin_folder() {
        let dir = std::env::temp_dir().join(format!("zs-verilator-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        assert_eq!(root_for(&dir.join("bin")), None);
        std::fs::create_dir_all(dir.join("share").join("verilator").join("include")).unwrap();
        assert_eq!(
            root_for(&dir.join("bin")),
            Some(dir.join("share").join("verilator"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
