use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Whether an engine is built in, external, or planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Implemented in Rust inside this workspace.
    Native,
    /// A separately installed program driven through an adapter.
    External,
}

/// Which side of the system a backend simulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendRole {
    /// Processing system (Cortex-A9 and PS peripherals).
    Ps,
    /// Programmable logic (RTL).
    Pl,
}

/// One entry of the backend inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendInfo {
    /// Name used in `[backends]` of `project.toml`.
    pub name: String,
    /// PS or PL.
    pub role: BackendRole,
    /// Built in or external.
    pub kind: BackendKind,
    /// True when this build contains the adapter and it can run.
    pub implemented: bool,
    /// For external engines: whether the program was found on `PATH`.
    pub found: Option<bool>,
    /// Where the program was found.
    pub path: Option<String>,
    /// Programs searched for.
    pub executables: Vec<String>,
    /// Milestone or note explaining the status.
    pub note: String,
}

/// Locates an executable on `PATH`, trying the platform's usual suffixes.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".bat", ".cmd"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for s in suffixes {
            let candidate = dir.join(format!("{name}{s}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Directories searched in addition to `PATH`: `XILINX_VITIS` / `XILINX_VIVADO`
/// and the conventional AMD/Xilinx install roots, one level of version
/// directories deep. Only layouts observed on a real install are listed.
fn extra_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["XILINX_VITIS", "XILINX_VIVADO"] {
        if let Some(v) = std::env::var_os(var) {
            roots.push(PathBuf::from(v));
        }
    }
    for fixed in [
        r"C:\Xilinx\Vitis",
        r"C:\Xilinx\Vivado",
        "/opt/Xilinx/Vitis",
        "/opt/Xilinx/Vivado",
        "/tools/Xilinx/Vitis",
        "/tools/Xilinx/Vivado",
    ] {
        roots.push(PathBuf::from(fixed));
    }
    let mut dirs = Vec::new();
    for root in roots {
        // A root may be a versioned install itself or contain versions.
        let mut installs = vec![root.clone()];
        if let Ok(rd) = std::fs::read_dir(&root) {
            installs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for i in installs {
            dirs.push(i.join("bin"));
            dirs.push(
                i.join("data")
                    .join("emulation")
                    .join("qemu_win")
                    .join("qemu"),
            );
        }
    }
    dirs
}

/// Locates an executable on `PATH` or in the Xilinx install directories,
/// accepting script wrappers (`.bat`, `.cmd`) as well as binaries.
pub fn locate(name: &str) -> Option<PathBuf> {
    locate_with(name, true)
}

/// Like [`locate`] but only real binaries: on Windows `.exe`, never a `.bat`
/// or `.cmd` launcher. Vitis ships such launchers next to the real QEMU, and a
/// wrapper cannot be driven or killed as the emulator process.
pub fn locate_binary(name: &str) -> Option<PathBuf> {
    locate_with(name, false)
}

fn locate_with(name: &str, scripts_ok: bool) -> Option<PathBuf> {
    let suffixes: &[&str] = match (cfg!(windows), scripts_ok) {
        (false, _) => &[""],
        (true, true) => &["", ".exe", ".bat", ".cmd"],
        (true, false) => &["", ".exe"],
    };
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    // Binaries first, everywhere, so a launcher script earlier on the search
    // path cannot shadow the real executable further along.
    let dirs: Vec<PathBuf> = path_dirs.into_iter().chain(extra_dirs()).collect();
    for s in suffixes {
        for d in &dirs {
            let candidate = d.join(format!("{name}{s}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The newest Vivado install directory (one that contains `settings64.bat`):
/// `XILINX_VIVADO`, then `C:\Xilinx\Vivado\<ver>` and `/opt|/tools/Xilinx/Vivado/<ver>`.
pub fn find_vivado() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("XILINX_VIVADO") {
        candidates.push(PathBuf::from(v));
    }
    for root in [
        r"C:\Xilinx\Vivado",
        "/opt/Xilinx/Vivado",
        "/tools/Xilinx/Vivado",
    ] {
        if let Ok(rd) = std::fs::read_dir(root) {
            candidates.extend(rd.flatten().map(|e| e.path()));
        }
    }
    candidates.retain(|p| p.join("settings64.bat").is_file() || p.join("settings64.sh").is_file());
    candidates.sort();
    candidates.pop()
}

/// The XSim DPI static library: `ZYNQSIM_DPI_LIB`, next to the running
/// executable, or `target/dpi/release` of the working tree.
///
/// It lives in its own target directory (`cargo xtask dpi`) because a normal
/// workspace build compiles the same crate *without* the `freestanding`
/// feature to the same file name, which would silently replace it with an
/// archive that exports nothing.
pub fn find_dpi_lib() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_DPI_LIB") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    find_dpi_lib_from(exe.as_deref(), cwd.as_deref())
}

/// The search of [`find_dpi_lib`] with the executable and working directory
/// given, so it can be tested: `lib/` next to the executable (the release
/// archive), the executable's own folder, then `target/dpi/release` of the
/// tree the executable or the working directory is in.
fn find_dpi_lib_from(
    exe: Option<&std::path::Path>,
    cwd: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let name = "libxsim_dpi.a";
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = exe.and_then(std::path::Path::parent) {
        // An unpacked release archive: `<dir>/zynqsim` and `<dir>/lib/libxsim_dpi.a`.
        dirs.push(d.join("lib"));
        dirs.push(d.to_path_buf());
        // `target/debug/deps/<exe>`, `target/release/<exe>`: the sibling
        // `target/dpi/release` is two or three levels up.
        for anc in d.ancestors().take(4) {
            dirs.push(anc.join("dpi").join("release"));
        }
    }
    if let Some(cwd) = cwd {
        for anc in cwd.ancestors().take(5) {
            dirs.push(anc.join("target").join("dpi").join("release"));
        }
    }
    // Only an archive that actually exports the DPI functions counts: a normal
    // build of the same crate has the same file name and exports nothing.
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| exports_dpi(p))
}

/// True when `path` is a readable archive containing the `rp_open` symbol.
fn exports_dpi(path: &std::path::Path) -> bool {
    std::fs::read(path)
        .map(|b| b.windows(7).any(|w| w == b"rp_open"))
        .unwrap_or(false)
}

fn external(
    name: &str,
    role: BackendRole,
    executables: &[&str],
    implemented: bool,
    binary_only: bool,
    note: &str,
) -> BackendInfo {
    let found_path = executables.iter().find_map(|e| {
        if binary_only {
            locate_binary(e)
        } else {
            locate(e)
        }
    });
    BackendInfo {
        name: name.to_string(),
        role,
        kind: BackendKind::External,
        implemented,
        found: Some(found_path.is_some()),
        path: found_path.map(|p| p.display().to_string()),
        executables: executables.iter().map(ToString::to_string).collect(),
        note: note.to_string(),
    }
}

/// The backend inventory for this build and this host.
///
/// External engines are located at run time (prompt §2.3); their absence is
/// reported, never fatal.
pub fn detect_backends() -> Vec<BackendInfo> {
    let native = |name: &str, role, implemented, note: &str| BackendInfo {
        name: name.to_string(),
        role,
        kind: BackendKind::Native,
        implemented,
        found: None,
        path: None,
        executables: Vec::new(),
        note: note.to_string(),
    };
    vec![
        native(
            "demo",
            BackendRole::Ps,
            true,
            "built-in scripted model, no CPU (fidelity: demo)",
        ),
        external(
            "qemu",
            BackendRole::Ps,
            &["qemu-system-aarch64"],
            true,
            true,
            "Xilinx QEMU, F0 + instruction-count timing; needs the Xilinx build and its co-sim DTB",
        ),
        native(
            "native",
            BackendRole::Ps,
            false,
            "native Cortex-A9 backend: milestone P5 (optional)",
        ),
        native(
            "demo",
            BackendRole::Pl,
            true,
            "no PL (or the demo model with the demo PS)",
        ),
        native(
            "native",
            BackendRole::Pl,
            true,
            "native Rust register-level PL models (transaction level, not cycle-accurate)",
        ),
        {
            // Located by the adapter crate: `verilator_bin` (the `verilator` launcher is Perl).
            let found = backend_rtl_verilator::tools::find();
            BackendInfo {
                name: "verilator".to_string(),
                role: BackendRole::Pl,
                kind: BackendKind::External,
                implemented: true,
                found: Some(found.is_some()),
                path: found.as_ref().map(|t| t.verilator.display().to_string()),
                executables: vec!["verilator_bin".to_string(), "verilator".to_string()],
                note:
                    "RTL in Verilator (--binary --timing) through the Rust DPI library; also needs \
                       make, a C++ compiler and sh; 2-state (docs/INSTALL.md)"
                        .to_string(),
            }
        },
        external(
            "icarus",
            BackendRole::Pl,
            &["iverilog", "vvp"],
            false,
            false,
            "RTL adapter: milestone P1/P4, not in this build",
        ),
        external(
            "xsim",
            BackendRole::Pl,
            &["xsim", "vivado"],
            true,
            false,
            "RTL in Vivado XSim via the Rust DPI library; Windows only for now",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_marks_exactly_the_built_backends_implemented() {
        let all = detect_backends();
        let implemented: Vec<_> = all.iter().filter(|b| b.implemented).collect();
        let names: Vec<(&str, BackendRole)> = implemented
            .iter()
            .map(|b| (b.name.as_str(), b.role))
            .collect();
        assert_eq!(
            names,
            [
                ("demo", BackendRole::Ps),
                ("qemu", BackendRole::Ps),
                ("demo", BackendRole::Pl),
                ("native", BackendRole::Pl),
                ("verilator", BackendRole::Pl),
                ("xsim", BackendRole::Pl),
            ]
        );
        for b in all.iter().filter(|b| b.kind == BackendKind::External) {
            assert!(b.found.is_some(), "{} must report found/not found", b.name);
        }
    }

    #[test]
    fn missing_program_is_none() {
        assert!(find_on_path("definitely-not-a-real-program-zynqsim").is_none());
        assert!(locate("definitely-not-a-real-program-zynqsim").is_none());
        assert!(locate_binary("definitely-not-a-real-program-zynqsim").is_none());
    }

    #[test]
    fn the_dpi_library_is_found_in_an_unpacked_release_archive() {
        let root = std::env::temp_dir().join(format!(
            "zynqsim-dpi-lookup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let lib_dir = root.join("lib");
        std::fs::create_dir_all(&lib_dir).unwrap();
        let lib = lib_dir.join("libxsim_dpi.a");
        // A file that exports nothing is not the library.
        std::fs::write(&lib, b"not an archive").unwrap();
        assert_eq!(find_dpi_lib_from(Some(&root.join("zynqsim")), None), None);
        std::fs::write(&lib, b"!<arch>\nrp_open\n").unwrap();
        assert_eq!(
            find_dpi_lib_from(Some(&root.join("zynqsim")), None),
            Some(lib.clone())
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
