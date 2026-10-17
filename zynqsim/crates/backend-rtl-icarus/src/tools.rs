//! Finding Icarus Verilog, its runtime `vvp` and the zynqsim VPI module.
//!
//! Order for the compiler (`iverilog.exe`): `ZYNQSIM_ICARUS` (a file, or a folder holding
//! `bin\iverilog.exe`), then `PATH`, then the default unpacked-archive folder
//! `<temp>/zs-tools/verilator/oss-cad-suite/bin` (the same archive Verilator uses; see
//! `docs/INSTALL.md`). The runtime `vvp.exe` is the one beside the compiler.
//!
//! The VPI module is `zynqsim.vpi`: `ZYNQSIM_VPI` (a file), then `target/vpi/release` of the
//! tree the executable or the working directory is in (`cargo xtask vpi`).

use std::path::{Path, PathBuf};

/// What was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcarusTools {
    /// The compiler, `iverilog.exe`.
    pub iverilog: PathBuf,
    /// The runtime, `vvp.exe` (beside the compiler).
    pub vvp: PathBuf,
    /// Directories put first on `PATH` (the DLLs of the suite: `bin` and `lib`).
    pub tool_dirs: Vec<PathBuf>,
}

/// The default unpacked-archive folder, relative to the temporary directory.
pub const DEFAULT_TOOLS_DIR: &[&str] = &["zs-tools", "verilator", "oss-cad-suite"];

/// `dir/name` when it is a file.
fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// Accepts the compiler file, or a folder that holds it (or a `bin` folder that does).
fn from_explicit(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if p.is_dir() {
        return in_dir(p, "iverilog.exe").or_else(|| in_dir(&p.join("bin"), "iverilog.exe"));
    }
    None
}

/// The first directory on `PATH` that holds `name.exe`.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(format!("{name}.exe")))
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

/// Finds Icarus Verilog. `None` when `iverilog.exe` (and `vvp.exe` beside it) is not found.
pub fn find() -> Option<IcarusTools> {
    let iverilog = std::env::var_os("ZYNQSIM_ICARUS")
        .and_then(|v| from_explicit(Path::new(&v)))
        .or_else(|| on_path("iverilog"))
        .or_else(|| in_dir(&default_suite()?.join("bin"), "iverilog.exe"))?;
    let bin = iverilog.parent()?.to_path_buf();
    let vvp = in_dir(&bin, "vvp.exe")?;
    let mut tool_dirs = vec![bin.clone()];
    if let Some(lib) = bin.parent().map(|p| p.join("lib")).filter(|p| p.is_dir()) {
        // The suite keeps the DLLs `libvvp-1.dll` needs (`libstdc++`, `libreadline`) in `lib`.
        tool_dirs.push(lib);
    }
    Some(IcarusTools {
        iverilog,
        vvp,
        tool_dirs,
    })
}

/// Finds `zynqsim.vpi`: `ZYNQSIM_VPI` (a file), then `target/vpi/release/zynqsim.vpi` of the
/// tree the executable or the working directory is in.
pub fn find_vpi() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_VPI") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = exe.as_deref().and_then(Path::parent) {
        dirs.push(d.to_path_buf());
        for anc in d.ancestors().take(4) {
            dirs.push(anc.join("vpi").join("release"));
        }
    }
    if let Some(cwd) = cwd.as_deref() {
        for anc in cwd.ancestors().take(5) {
            dirs.push(anc.join("target").join("vpi").join("release"));
        }
    }
    dirs.into_iter()
        .map(|d| d.join("zynqsim.vpi"))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_folder_resolves_to_the_compiler() {
        let dir = std::env::temp_dir().join(format!("zs-icarus-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        assert_eq!(from_explicit(&dir), None);
        std::fs::write(dir.join("bin").join("iverilog.exe"), b"MZ").unwrap();
        assert_eq!(
            from_explicit(&dir),
            Some(dir.join("bin").join("iverilog.exe"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
