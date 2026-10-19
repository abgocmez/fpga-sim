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
            true,
            "native Cortex-A9 interpreter (cpu-a9, F0, A32/Thumb/VFP, PS + PL, dual-core)",
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
        native(
            "rtl-native",
            BackendRole::Pl,
            true,
            "pure-Rust RTL simulation engine (rtl-engine, 2-state/4-state, multi-slave, zero external tools)",
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
        {
            // Located by the adapter crate: `iverilog.exe` (its `vvp.exe` beside it), and the
            // zynqsim VPI module that is built by `cargo xtask vpi`.
            let found = backend_rtl_icarus::tools::find();
            let vpi = backend_rtl_icarus::tools::find_vpi();
            BackendInfo {
                name: "icarus".to_string(),
                role: BackendRole::Pl,
                kind: BackendKind::External,
                implemented: true,
                found: Some(found.is_some() && vpi.is_some()),
                path: found.as_ref().map(|t| t.iverilog.display().to_string()),
                executables: vec!["iverilog".to_string(), "vvp".to_string()],
                note: format!(
                    "RTL in Icarus Verilog (iverilog -g2012, vvp) through the zynqsim VPI module \
                     (ADR-037); module: {}; Windows only for now",
                    vpi.map(|p| p.display().to_string())
                        .unwrap_or_else(|| "not built (cargo xtask vpi)".to_string())
                ),
            }
        },
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

/// Operating system and CPU architecture of the host machine running ZynqSim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// Host operating system (`std::env::consts::OS`).
    pub os: String,
    /// Host CPU architecture (`std::env::consts::ARCH`).
    pub arch: String,
    /// Host OS family (`std::env::consts::FAMILY`).
    pub family: String,
}

/// Status of an external tool or compiler on the host system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    /// Name of the tool or executable (e.g. `cargo`, `qemu`, `xsim`).
    pub name: String,
    /// Category of tool (e.g. `toolchain`, `compiler`, `simulator`, `eda`, `bridge`).
    pub category: String,
    /// True if the tool or executable was located on the system.
    pub found: bool,
    /// Absolute path to the located executable or library.
    pub path: Option<String>,
    /// Version string reported by the tool (if queried).
    pub version: Option<String>,
    /// Helpful note or guidance regarding the tool.
    pub note: String,
}

/// Status and readiness of a PS or PL simulation route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteStatus {
    /// Name of the engine or route (`native`, `rtl-native`, `qemu`, `xsim`, etc.).
    pub name: String,
    /// PS or PL.
    pub role: BackendRole,
    /// Whether the route has all prerequisites to run.
    pub ready: bool,
    /// Summary describing the route's capabilities.
    pub summary: String,
    /// List of prerequisites or tools required for this route.
    pub requirements: Vec<String>,
}

/// High-level readiness verdict and actionable guidance for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorSummary {
    /// Whether the pure-Rust native simulation route is ready (always true).
    pub native_route_ready: bool,
    /// Whether the Xilinx QEMU PS simulation route is ready.
    pub qemu_route_ready: bool,
    /// Whether the Vivado XSim PL simulation route is ready.
    pub xsim_route_ready: bool,
    /// Whether the Verilator PL simulation route is ready.
    pub verilator_route_ready: bool,
    /// Whether the Icarus Verilog PL simulation route is ready.
    pub icarus_route_ready: bool,
    /// Human-readable verdict summarizing overall readiness.
    pub verdict: String,
    /// Actionable next steps or recommendations based on detected tools.
    pub recommendations: Vec<String>,
}

/// A comprehensive diagnostic report produced by [`diagnose`] (and `zynqsim doctor`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Host platform details.
    pub host: HostInfo,
    /// Inventory of built-in and external simulation backends.
    pub backends: Vec<BackendInfo>,
    /// Status of compilers, build utilities, and EDA tools.
    pub tools: Vec<ToolStatus>,
    /// Readiness of each simulation route.
    pub routes: Vec<RouteStatus>,
    /// High-level summary, verdict, and recommendations.
    pub summary: DoctorSummary,
}

impl DoctorReport {
    /// Formats the diagnostic report as clear, human-readable text.
    pub fn format_text(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "================================================================================\n",
        );
        out.push_str(
            "                           ZynqSim Doctor Diagnostic                            \n",
        );
        out.push_str(
            "================================================================================\n\n",
        );

        out.push_str("Host:\n");
        out.push_str(&format!(
            "  OS: {} ({}, {})\n\n",
            self.host.os, self.host.arch, self.host.family
        ));

        out.push_str("Compilers & Toolchains:\n");
        for t in self.tools.iter().filter(|t| {
            t.category == "toolchain" || t.category == "compiler" || t.category == "build"
        }) {
            let status = if t.found { "[FOUND]  " } else { "[MISSING]" };
            let detail = match (&t.path, &t.version) {
                (Some(p), Some(v)) => format!("{p} ({v})"),
                (Some(p), None) => p.clone(),
                (None, _) => t.note.clone(),
            };
            out.push_str(&format!("  {:<10} {} {}\n", t.name, status, detail));
        }
        out.push('\n');

        out.push_str("EDA & External Simulators:\n");
        for t in self.tools.iter().filter(|t| {
            t.category != "toolchain" && t.category != "compiler" && t.category != "build"
        }) {
            let status = if t.found { "[FOUND]  " } else { "[MISSING]" };
            let detail = match (&t.path, &t.version) {
                (Some(p), Some(v)) => format!("{p} ({v})"),
                (Some(p), None) => p.clone(),
                (None, _) => t.note.clone(),
            };
            out.push_str(&format!("  {:<10} {} {}\n", t.name, status, detail));
        }
        out.push('\n');

        out.push_str("Simulation Routes:\n");
        for r in &self.routes {
            let state = if r.ready { "[READY]" } else { "[SKIP ]" };
            let role = match r.role {
                BackendRole::Ps => "PS",
                BackendRole::Pl => "PL",
            };
            out.push_str(&format!(
                "  {} {:<20} ({role}): {}\n",
                state, r.name, r.summary
            ));
        }
        out.push('\n');

        out.push_str("Verdict:\n");
        out.push_str(&format!("  {}\n\n", self.summary.verdict));

        if !self.summary.recommendations.is_empty() {
            out.push_str("Recommendations & Next Steps:\n");
            for rec in &self.summary.recommendations {
                out.push_str(&format!("  * {rec}\n"));
            }
            out.push('\n');
        }

        out.push_str(
            "================================================================================\n",
        );
        out
    }
}

fn tool_version(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    let text = if !output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        String::from_utf8_lossy(&output.stderr)
    };
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Diagnoses the host environment, toolchains, EDA simulators and backend readiness.
pub fn diagnose() -> DoctorReport {
    let host = HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        family: std::env::consts::FAMILY.to_string(),
    };

    let backends = detect_backends();

    let mut tools = Vec::new();

    // 1. Cargo
    let cargo_p = find_on_path("cargo");
    let cargo_v = cargo_p.as_deref().and_then(tool_version);
    tools.push(ToolStatus {
        name: "cargo".to_string(),
        category: "toolchain".to_string(),
        found: cargo_p.is_some(),
        path: cargo_p.map(|p| p.display().to_string()),
        version: cargo_v,
        note: "Rust package manager & runner (install via rustup)".to_string(),
    });

    // 2. Rustc
    let rustc_p = find_on_path("rustc");
    let rustc_v = rustc_p.as_deref().and_then(tool_version);
    tools.push(ToolStatus {
        name: "rustc".to_string(),
        category: "toolchain".to_string(),
        found: rustc_p.is_some(),
        path: rustc_p.map(|p| p.display().to_string()),
        version: rustc_v,
        note: "Rust compiler (install via rustup)".to_string(),
    });

    // 3. C/C++ compiler
    let cc_candidates: &[&str] = if cfg!(windows) {
        &["gcc", "clang", "cl"]
    } else {
        &["cc", "gcc", "clang"]
    };
    let cc_p = cc_candidates.iter().find_map(|&c| find_on_path(c));
    let cc_v = cc_p.as_deref().and_then(tool_version);
    tools.push(ToolStatus {
        name: "cc/c++".to_string(),
        category: "compiler".to_string(),
        found: cc_p.is_some(),
        path: cc_p.map(|p| p.display().to_string()),
        version: cc_v,
        note: "C/C++ compiler (gcc, clang, or cl; needed for Verilator/VPI/DPI)".to_string(),
    });

    // 4. Make
    let make_candidates: &[&str] = if cfg!(windows) {
        &["make", "mingw32-make", "gmake", "nmake"]
    } else {
        &["make", "gmake"]
    };
    let make_p = make_candidates.iter().find_map(|&m| find_on_path(m));
    tools.push(ToolStatus {
        name: "make".to_string(),
        category: "build".to_string(),
        found: make_p.is_some(),
        path: make_p.map(|p| p.display().to_string()),
        version: None,
        note: "make utility (needed for Verilator builds)".to_string(),
    });

    // 5. QEMU
    let qemu_p = locate_binary("qemu-system-aarch64");
    let qemu_found = qemu_p.is_some();
    tools.push(ToolStatus {
        name: "qemu".to_string(),
        category: "simulator".to_string(),
        found: qemu_found,
        path: qemu_p.map(|p| p.display().to_string()),
        version: None,
        note: "Xilinx QEMU (Vitis) for Cortex-A9 functional emulation".to_string(),
    });

    // 6. Vivado
    let vivado_p = find_vivado();
    let vivado_found = vivado_p.is_some();
    tools.push(ToolStatus {
        name: "vivado".to_string(),
        category: "eda".to_string(),
        found: vivado_found,
        path: vivado_p.map(|p| p.display().to_string()),
        version: None,
        note: "Vivado Design Suite (for XSim, IP generation, XSA hardware handoffs)".to_string(),
    });

    // 7. XSim
    let xsim_p = locate("xsim");
    let xsim_found = xsim_p.is_some();
    tools.push(ToolStatus {
        name: "xsim".to_string(),
        category: "simulator".to_string(),
        found: xsim_found,
        path: xsim_p.map(|p| p.display().to_string()),
        version: None,
        note: "Vivado Logic Simulator for RTL co-simulation".to_string(),
    });

    // 8. DPI library
    let dpi_p = find_dpi_lib();
    let dpi_found = dpi_p.is_some();
    tools.push(ToolStatus {
        name: "dpi-lib".to_string(),
        category: "bridge".to_string(),
        found: dpi_found,
        path: dpi_p.map(|p| p.display().to_string()),
        version: None,
        note: "ZynqSim DPI static library for XSim co-simulation (build with `cargo xtask dpi`)"
            .to_string(),
    });

    // 9. Verilator
    let verilator_t = backend_rtl_verilator::tools::find();
    let verilator_found = verilator_t.is_some();
    tools.push(ToolStatus {
        name: "verilator".to_string(),
        category: "simulator".to_string(),
        found: verilator_found,
        path: verilator_t.map(|t| t.verilator.display().to_string()),
        version: None,
        note: "Verilator C++ cycle simulator for RTL".to_string(),
    });

    // 10. Icarus Verilog
    let icarus_t = backend_rtl_icarus::tools::find();
    let icarus_found = icarus_t.is_some();
    tools.push(ToolStatus {
        name: "iverilog".to_string(),
        category: "simulator".to_string(),
        found: icarus_found,
        path: icarus_t.map(|t| t.iverilog.display().to_string()),
        version: None,
        note: "Icarus Verilog simulator for RTL".to_string(),
    });

    // 11. VPI module
    let vpi_p = backend_rtl_icarus::tools::find_vpi();
    let vpi_found = vpi_p.is_some();
    tools.push(ToolStatus {
        name: "vpi-mod".to_string(),
        category: "bridge".to_string(),
        found: vpi_found,
        path: vpi_p.map(|p| p.display().to_string()),
        version: None,
        note: "ZynqSim VPI module for Icarus (build with `cargo xtask vpi`)".to_string(),
    });

    let xsim_ready = xsim_found && dpi_found;
    let icarus_ready = icarus_found && vpi_found;

    let routes = vec![
        RouteStatus {
            name: "native".to_string(),
            role: BackendRole::Ps,
            ready: true,
            summary: "Cortex-A9 native interpreter (A32/Thumb-2/VFPv3, dual-core, PS+PL co-sim)".to_string(),
            requirements: vec!["Built-in (pure Rust, zero external dependencies)".to_string()],
        },
        RouteStatus {
            name: "native".to_string(),
            role: BackendRole::Pl,
            ready: true,
            summary: "Transaction-level vendor IP models (GPIO, Timer, UART Lite, DMA, IIC, BRAM, INTC, SPI, AXIS FIFO, Clk Wiz)".to_string(),
            requirements: vec!["Built-in (pure Rust, zero external dependencies)".to_string()],
        },
        RouteStatus {
            name: "rtl-native".to_string(),
            role: BackendRole::Pl,
            ready: true,
            summary: "Pure-Rust RTL simulation engine (rtl-engine, 2-state/4-state, multi-slave)".to_string(),
            requirements: vec!["Built-in (pure Rust, zero external dependencies)".to_string()],
        },
        RouteStatus {
            name: "qemu".to_string(),
            role: BackendRole::Ps,
            ready: qemu_found,
            summary: "Xilinx QEMU co-simulation (F0 instruction-timing)".to_string(),
            requirements: vec!["qemu-system-aarch64 on PATH or in Xilinx directories".to_string()],
        },
        RouteStatus {
            name: "xsim".to_string(),
            role: BackendRole::Pl,
            ready: xsim_ready,
            summary: "Vivado XSim co-simulation via DPI".to_string(),
            requirements: vec![
                "Vivado XSim on PATH or in Xilinx directories".to_string(),
                "libxsim_dpi.a (cargo xtask dpi)".to_string(),
            ],
        },
        RouteStatus {
            name: "verilator".to_string(),
            role: BackendRole::Pl,
            ready: verilator_found,
            summary: "Verilator RTL simulation (--binary --timing)".to_string(),
            requirements: vec![
                "verilator / verilator_bin on PATH".to_string(),
                "C++ compiler and make".to_string(),
            ],
        },
        RouteStatus {
            name: "icarus".to_string(),
            role: BackendRole::Pl,
            ready: icarus_ready,
            summary: "Icarus Verilog RTL simulation via VPI".to_string(),
            requirements: vec![
                "iverilog and vvp on PATH".to_string(),
                "zynqsim.vpi (cargo xtask vpi)".to_string(),
            ],
        },
    ];

    let mut recommendations = Vec::new();
    if !qemu_found {
        recommendations.push(
            "Xilinx QEMU is not installed/detected: use `backends.ps = \"native\"` (or `zynqsim init --xsa ... --ps native`) to simulate Cortex-A9 PS firmware directly in pure Rust."
                .to_string(),
        );
    }
    if !xsim_ready && !verilator_found && !icarus_ready {
        recommendations.push(
            "No external RTL simulator detected: use `backends.pl = \"native\"` (vendor IP models) or `backends.pl = \"rtl-native\"` (pure-Rust RTL simulation) with zero external tools."
                .to_string(),
        );
    }
    if xsim_found && !dpi_found {
        recommendations.push(
            "Vivado XSim was found, but the DPI library is missing: run `cargo xtask dpi` to build `libxsim_dpi.a`."
                .to_string(),
        );
    }
    if icarus_found && !vpi_found {
        recommendations.push(
            "Icarus Verilog was found, but the VPI module is missing: run `cargo xtask vpi` to build `zynqsim.vpi`."
                .to_string(),
        );
    }
    recommendations
        .push("Try the automated first-run walkthrough: run `cargo xtask try-xsa`".to_string());
    recommendations.push(
        "Or run the quick demo tour: `.\\demo.cmd --quick` (PowerShell) / `cargo xtask demo --quick`".to_string(),
    );

    let verdict = if qemu_found && xsim_ready {
        "Ready for both Pure-Rust Native simulation and Xilinx EDA co-simulation (QEMU + Vivado XSim).".to_string()
    } else {
        "Ready for Pure-Rust Native simulation (PS: native Cortex-A9 interpreter with A32/Thumb/VFP, PL: vendor-ip models & rtl-engine). Zero Vivado or QEMU installation required!".to_string()
    };

    let summary = DoctorSummary {
        native_route_ready: true,
        qemu_route_ready: qemu_found,
        xsim_route_ready: xsim_ready,
        verilator_route_ready: verilator_found,
        icarus_route_ready: icarus_ready,
        verdict,
        recommendations,
    };

    DoctorReport {
        host,
        backends,
        tools,
        routes,
        summary,
    }
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
                ("native", BackendRole::Ps),
                ("demo", BackendRole::Pl),
                ("native", BackendRole::Pl),
                ("rtl-native", BackendRole::Pl),
                ("verilator", BackendRole::Pl),
                ("icarus", BackendRole::Pl),
                ("xsim", BackendRole::Pl),
            ]
        );
        for b in all.iter().filter(|b| b.kind == BackendKind::External) {
            assert!(b.found.is_some(), "{} must report found/not found", b.name);
        }
    }

    #[test]
    fn diagnose_reports_native_route_ready_on_any_host() {
        let report = diagnose();
        assert!(report.summary.native_route_ready);
        assert!(
            report
                .routes
                .iter()
                .any(|r| r.name == "native" && r.role == BackendRole::Ps && r.ready)
        );
        assert!(
            report
                .routes
                .iter()
                .any(|r| r.name == "rtl-native" && r.role == BackendRole::Pl && r.ready)
        );
        // Acceptance requirement: output on a machine with no Xilinx tools names the native route.
        assert!(
            report.summary.verdict.contains("Pure-Rust Native")
                || report.summary.verdict.contains("both Pure-Rust Native")
        );
    }

    #[test]
    fn diagnose_format_text_contains_key_sections_and_native_route() {
        let report = diagnose();
        let text = report.format_text();
        assert!(text.contains("ZynqSim Doctor Diagnostic"));
        assert!(text.contains("Cortex-A9 native interpreter"));
        assert!(text.contains("Pure-Rust RTL simulation engine"));
        assert!(text.contains("Verdict:"));
    }

    #[test]
    fn diagnose_serializes_cleanly_to_json() {
        let report = diagnose();
        let val = serde_json::to_value(&report).expect("must serialize");
        assert!(val["summary"]["native_route_ready"].as_bool().unwrap());
        assert!(!val["routes"].as_array().unwrap().is_empty());
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
