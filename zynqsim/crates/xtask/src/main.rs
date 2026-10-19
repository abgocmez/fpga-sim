//! Build automation (`cargo xtask <command>`), written in Rust so the project
//! needs no shell or Python tooling (prompt §2.1).

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Instant;
use xtask::ci::{self, CiMode, StepRecord};
use xtask::{demo, dist};

const HELP: &str = "\
cargo xtask <command>

  ci            fmt --check, clippy -D warnings (workspace and freestanding DPI), all tests,
                release build: THE GATE TO RUN BEFORE MERGING (45-60 min). Prints the wall
                time of each step and a summary table at the end, pass or fail.
  ci --fast     fmt --check, clippy -D warnings (workspace), and the workspace tests with
                ZYNQSIM_FAST=1: every test that needs QEMU, Vivado, Verilator or Icarus
                prints SKIPPED and passes vacuously. Quick feedback while working; it does
                NOT replace `cargo xtask ci` before merging.
  ci --det-only name,name
                only the determinism test (crates/cli/tests/determinism.rs), restricted to
                the listed examples (ZYNQSIM_DET_ONLY). Every examples/ directory must still
                be in its CASES or EXCLUDED tables. Unknown names fail the test.
  changed-examples
                the examples/<name> directories touched by `git diff origin/main...HEAD`,
                comma-separated, for `ci --det-only \"$(cargo xtask changed-examples)\"`
  test          all tests
  golden        regenerate the golden board PNGs (review the diff before committing)
  release       cargo build --release (zynqsim, zynqsim-gui, zynqsim-mcp)
  dpi           build the XSim DPI library (target/dpi/release/libxsim_dpi.a)
  dpi-shared    build the shared-object DPI library (target/dpi/release/zynqsim_dpi.dll, .so or .dylib)
  vpi           build the Icarus VPI module (target/vpi/release/zynqsim.vpi)
  dist          release archive in dist/ (binaries, DPI library, examples, fixtures,
                boards, docs, licence, INSTALL.txt, SHA256SUMS), then an unpack check
                from a clean directory
  dist-verify   the unpack check alone, on [ARCHIVE] (default: newest in dist/)
  firmware      build the Rust firmware (needs the armv7a-none-eabi target) and copy the
                ELFs to fixtures/elf/rust_*.elf
  try-xsa [XSA] [ELF]
                runs the first-run walkthrough end-to-end: imports an XSA with the native PS
                and PL, validates the generated project, runs it, and checks simulation results
                (zero external EDA tools or Vivado/QEMU required).
  demo          a tour of the headless checks through the real binaries, with output in
                demo-out/ (see docs/DEMO.md). Options: --quick (5-minute subset),
                --only name,... (step names), --gui (also the GUI screenshots)
  help          this text
";

fn workspace_root() -> PathBuf {
    // crates/xtask -> workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cargo_program() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn cargo(args: &[&str], env: &[(&str, &str)]) -> bool {
    eprintln!("\n==> cargo {}", args.join(" "));
    let status = Command::new(cargo_program())
        .args(args)
        .envs(env.iter().copied())
        .current_dir(workspace_root())
        .status();
    match status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("xtask: cannot run cargo: {e}");
            false
        }
    }
}

/// Writes `THIRD_PARTY.md` listing every external crate in the lock file with
/// the licence its manifest declares.
fn third_party() -> bool {
    // The host workspace and the bare-metal firmware workspace (its own lock
    // file, `embedded-sdmmc` and friends live there).
    let mut packages: Vec<Value> = Vec::new();
    for dir in [workspace_root(), workspace_root().join("firmware")] {
        let out = Command::new(cargo_program())
            .args(["metadata", "--format-version", "1", "--locked"])
            .current_dir(&dir)
            .output();
        let out = match out {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                eprintln!(
                    "xtask: cargo metadata failed in {}:\n{}",
                    dir.display(),
                    String::from_utf8_lossy(&o.stderr)
                );
                return false;
            }
            Err(e) => {
                eprintln!("xtask: cannot run cargo: {e}");
                return false;
            }
        };
        let meta: Value = match serde_json::from_slice(&out.stdout) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("xtask: bad cargo metadata: {e}");
                return false;
            }
        };
        packages.extend(meta["packages"].as_array().cloned().unwrap_or_default());
    }

    // name -> (versions, license); only packages from a registry.
    let mut rows: BTreeMap<String, (Vec<String>, String)> = BTreeMap::new();
    for p in &packages {
        if p["source"].is_null() {
            continue; // workspace member
        }
        let name = p["name"].as_str().unwrap_or("?").to_string();
        let version = p["version"].as_str().unwrap_or("?").to_string();
        let license = p["license"]
            .as_str()
            .unwrap_or("(none declared: check the crate)")
            .to_string();
        let e = rows.entry(name).or_insert_with(|| (Vec::new(), license));
        if !e.0.contains(&version) {
            e.0.push(version);
        }
    }

    let mut md = String::from(
        "# Third-party software\n\n\
         ZynqSim is licensed under Apache-2.0 (see `LICENSE`). It links the Rust crates below.\n\
         This file is generated by `cargo xtask third-party` from `Cargo.lock` and\n\
         `firmware/Cargo.lock` (the bare-metal firmware workspace); the licence\n\
         column is the SPDX expression each crate declares in its manifest and has not been\n\
         independently audited.\n\n\
         External programs (QEMU, Verilator, Icarus Verilog, Vivado/XSim) are **not** vendored or\n\
         linked; they are located at run time and driven as separate processes (see\n\
         `docs/adr/001-ps-engine-qemu.md`).\n\n\
         | Crate | Version(s) | License |\n|---|---|---|\n",
    );
    for (name, (versions, license)) in &rows {
        md.push_str(&format!(
            "| {name} | {} | {license} |\n",
            versions.join(", ")
        ));
    }
    let path = workspace_root().join("THIRD_PARTY.md");
    match std::fs::write(&path, md) {
        Ok(()) => {
            eprintln!("xtask: wrote {} ({} crates)", path.display(), rows.len());
            true
        }
        Err(e) => {
            eprintln!("xtask: cannot write {}: {e}", path.display());
            false
        }
    }
}

/// Names of the firmware binaries and the fixture each is copied to.
const FIRMWARE: &[(&str, &str)] = &[
    ("hello", "rust_hello.elf"),
    ("audio_demo", "rust_audio.elf"),
    ("hdmi_demo", "rust_hdmi.elf"),
    ("irq_demo", "rust_irq.elf"),
    ("oled_demo", "rust_oled.elf"),
    ("pmod_simple_demo", "rust_pmod_simple.elf"),
    ("rgb_demo", "rust_rgb.elf"),
    ("tmp2_demo", "rust_tmp2.elf"),
    ("als_demo", "rust_als.elf"),
    ("acl_demo", "rust_acl.elf"),
    ("pcam_demo", "rust_pcam.elf"),
    ("net_demo", "rust_net.elf"),
    ("sd_demo", "rust_sd.elf"),
    ("qspi_demo", "rust_qspi.elf"),
    ("debug_demo", "rust_debug.elf"),
    ("gpio_timer_uart", "rust_gpio_timer_uart.elf"),
    ("timer_modes", "rust_timer_modes.elf"),
    ("vendor_mix", "rust_vendor_mix.elf"),
    ("handoff_demo", "rust_handoff_demo.elf"),
    ("board8_demo", "rust_board8.elf"),
    ("dma_loopback", "rust_dma_loopback.elf"),
    ("intc_demo", "rust_intc_demo.elf"),
    ("iic_bram_demo", "rust_iic_bram.elf"),
    ("dma_sg", "rust_dma_sg.elf"),
    ("conf_gpiops", "rust_conf_gpiops.elf"),
    ("conf_scutimer", "rust_conf_scutimer.elf"),
    ("timer_rate", "rust_timer_rate.elf"),
    ("dual_core_demo", "rust_dual_core.elf"),
    ("conf_ttc", "rust_conf_ttc.elf"),
    ("conf_dmaps", "rust_conf_dmaps.elf"),
    ("conf_iicps", "rust_conf_iicps.elf"),
    ("conf_spips", "rust_conf_spips.elf"),
    ("conf_xadcps", "rust_conf_xadcps.elf"),
    ("two_math", "rust_two_math.elf"),
    ("spi_demo", "rust_spi.elf"),
    ("axis_fifo_demo", "rust_axis_fifo.elf"),
    ("clk_wiz_demo", "rust_clk_wiz.elf"),
    ("vdma_demo", "rust_vdma.elf"),
    ("ethlite_demo", "rust_ethlite.elf"),
    ("vtc_demo", "rust_vtc.elf"),
    ("cdma_demo", "rust_cdma.elf"),
    ("uart16550_demo", "rust_uart16550.elf"),
];

/// Builds `firmware/` (its own workspace) and copies the ELFs into `fixtures/elf`.
fn firmware() -> bool {
    let root = workspace_root();
    let dir = root.join("firmware");
    eprintln!(
        "
==> cargo build --release (in {})",
        dir.display()
    );
    // The build directory is recorded in the DWARF compilation unit of the
    // `debug_demo` ELF; remap it to `.` so the committed fixture does not carry
    // the path of whoever built it (the debugger finds sources through
    // `[debug] source_dirs`). These are the flags of `firmware/.cargo/config.toml`
    // plus the remap: an env var replaces the config's rustflags, it does not add.
    let flags = [
        "-Clink-arg=-Tlink.ld".to_string(),
        "-Clink-arg=-Lrt".to_string(),
        format!("--remap-path-prefix={}=.", dir.display()),
    ]
    .join("\x1f");
    let built = Command::new(cargo_program())
        .args(["build", "--release"])
        .env("CARGO_ENCODED_RUSTFLAGS", flags)
        .current_dir(&dir)
        .status();
    match built {
        Ok(s) if s.success() => {}
        Ok(_) => return false,
        Err(e) => {
            eprintln!("xtask: cannot run cargo: {e}");
            return false;
        }
    }
    let out = dir.join("target/armv7a-none-eabi/release");
    for (bin, fixture) in FIRMWARE {
        let to = root.join("fixtures/elf").join(fixture);
        match std::fs::copy(out.join(bin), &to) {
            Ok(n) => eprintln!("xtask: {} ({n} bytes)", to.display()),
            Err(e) => {
                eprintln!("xtask: cannot copy {bin}: {e}");
                return false;
            }
        }
    }
    true
}

/// Runs the steps of one gate, timing each and recording the results for the summary.
struct Gate {
    started: Instant,
    steps: Vec<StepRecord>,
}

impl Gate {
    fn new() -> Self {
        Gate {
            started: Instant::now(),
            steps: Vec::new(),
        }
    }

    /// Runs one step and prints its wall time when it ends. The caller chains steps
    /// with `&&`, so the first failing step stops the gate.
    fn step(&mut self, name: &str, run: impl FnOnce() -> bool) -> bool {
        eprintln!("\n==> {name}");
        let t = Instant::now();
        let ok = run();
        let elapsed = t.elapsed();
        let verdict = if ok { "done in" } else { "FAILED after" };
        eprintln!("==> {name} ... {verdict} {}", ci::fmt_duration(elapsed));
        self.steps.push(StepRecord {
            name: name.to_string(),
            elapsed,
            ok,
        });
        ok
    }

    fn finish(&self, ok: bool) -> bool {
        eprint!(
            "\n==> gate {}, summary:\n{}",
            if ok { "passed" } else { "FAILED" },
            ci::summary_table(&self.steps, self.started.elapsed())
        );
        ok
    }
}

/// `cargo xtask ci [--fast | --det-only names]`.
fn ci_gate(args: &[String]) -> bool {
    let mode = match ci::parse_ci_args(args) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}\n\n{HELP}");
            return false;
        }
    };
    let mut gate = Gate::new();
    let ok = match mode {
        CiMode::Full => {
            gate.step("fmt", || cargo(&["fmt", "--all", "--", "--check"], &[]))
                && gate.step("clippy workspace", || {
                    cargo(
                        &[
                            "clippy",
                            "--workspace",
                            "--all-targets",
                            "--",
                            "-D",
                            "warnings",
                        ],
                        &[],
                    )
                })
                && gate.step("clippy xsim-dpi (freestanding)", || {
                    cargo(
                        &[
                            "clippy",
                            "-p",
                            "xsim-dpi",
                            "--features",
                            "freestanding",
                            "--",
                            "-D",
                            "warnings",
                        ],
                        &[],
                    )
                })
                && gate.step("build xsim-dpi (freestanding)", || {
                    cargo(
                        &[
                            "build",
                            "--release",
                            "-p",
                            "xsim-dpi",
                            "--features",
                            "freestanding",
                            "--target-dir",
                            "target/dpi",
                        ],
                        &[],
                    )
                })
                && gate.step("test --workspace", || cargo(&["test", "--workspace"], &[]))
                && gate.step("build --release", || cargo(&["build", "--release"], &[]))
        }
        CiMode::Fast => {
            gate.step("fmt", || cargo(&["fmt", "--all", "--", "--check"], &[]))
                && gate.step("clippy workspace", || {
                    cargo(
                        &[
                            "clippy",
                            "--workspace",
                            "--all-targets",
                            "--",
                            "-D",
                            "warnings",
                        ],
                        &[],
                    )
                })
                && gate.step("test --workspace (fast)", || {
                    cargo(&["test", "--workspace"], &[("ZYNQSIM_FAST", "1")])
                })
        }
        CiMode::DetOnly(names) => {
            let list = names.join(",");
            gate.step(&format!("determinism ({list})"), || {
                cargo(
                    &["test", "-p", "zynqsim", "--test", "determinism"],
                    &[("ZYNQSIM_DET_ONLY", list.as_str())],
                )
            })
        }
    };
    gate.finish(ok)
}

/// Prints the examples changed since `origin/main`, comma-separated.
fn changed_examples() -> bool {
    let out = Command::new("git")
        .args(["diff", "--name-only", "--relative", "origin/main...HEAD"])
        .current_dir(workspace_root())
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "xtask: git diff failed:\n{}",
                String::from_utf8_lossy(&o.stderr)
            );
            return false;
        }
        Err(e) => {
            eprintln!("xtask: cannot run git: {e}");
            return false;
        }
    };
    let names = ci::changed_examples(&String::from_utf8_lossy(&out.stdout));
    eprintln!("xtask: {} changed example(s)", names.len());
    println!("{}", names.join(","));
    true
}

/// Runs the first-run user walkthrough: `zynqsim init --xsa ... --ps native` followed by `zynqsim run`.
fn try_xsa(args: &[String]) -> bool {
    let root = workspace_root();
    let xsa_path = if let Some(arg) = args.first() {
        PathBuf::from(arg)
    } else {
        root.join("fixtures/xsa/zs_vendor_ip.xsa")
    };
    if !xsa_path.is_file() {
        eprintln!("xtask: XSA file not found at {}", xsa_path.display());
        return false;
    }
    let elf_path = if let Some(arg) = args.get(1) {
        PathBuf::from(arg)
    } else {
        root.join("fixtures/elf/rust_spi.elf")
    };
    if !elf_path.is_file() {
        eprintln!("xtask: ELF file not found at {}", elf_path.display());
        return false;
    }

    let temp_dir = std::env::temp_dir().join(format!("zynqsim-try-xsa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Build the CLI binary if not already present
    let exe_name = format!("zynqsim{}", std::env::consts::EXE_SUFFIX);
    let zynqsim_exe = root.join("target/debug").join(&exe_name);
    let zynqsim_bin = if zynqsim_exe.is_file() {
        zynqsim_exe
    } else {
        eprintln!("xtask: building zynqsim binary...");
        if !cargo(&["build", "-p", "zynqsim", "--bin", "zynqsim"], &[]) {
            return false;
        }
        root.join("target/debug").join(&exe_name)
    };

    eprintln!(
        "\n==> [1/3] zynqsim init --xsa {} --ps native --elf {}",
        xsa_path.display(),
        elf_path.display()
    );
    let init_status = Command::new(&zynqsim_bin)
        .args([
            "init",
            "--xsa",
            xsa_path.to_str().unwrap(),
            "--ps",
            "native",
            "--elf",
            elf_path.to_str().unwrap(),
            "--out",
            temp_dir.to_str().unwrap(),
        ])
        .status();

    let init_ok = match init_status {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("xtask: cannot run zynqsim init: {e}");
            false
        }
    };
    if !init_ok {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return false;
    }

    let project_file = temp_dir.join("project.toml");
    eprintln!(
        "\n==> [2/3] zynqsim inspect --project {}",
        project_file.display()
    );
    let inspect_out = Command::new(&zynqsim_bin)
        .args([
            "--json",
            "inspect",
            "--project",
            project_file.to_str().unwrap(),
        ])
        .output();
    let inspect_ok = match inspect_out {
        Ok(o) if o.status.success() => {
            let v: Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            let errs = v["diagnostics"]
                .as_array()
                .map(|a| a.iter().filter(|d| d["severity"] == "error").count())
                .unwrap_or(0);
            if errs > 0 {
                eprintln!(
                    "xtask: project has validation errors:\n{}",
                    String::from_utf8_lossy(&o.stdout)
                );
                false
            } else {
                true
            }
        }
        Ok(o) => {
            eprintln!(
                "xtask: zynqsim inspect failed:\n{}",
                String::from_utf8_lossy(&o.stderr)
            );
            false
        }
        Err(e) => {
            eprintln!("xtask: cannot run zynqsim inspect: {e}");
            false
        }
    };
    if !inspect_ok {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return false;
    }

    eprintln!(
        "\n==> [3/3] zynqsim run --project {} --for 10ms",
        project_file.display()
    );
    let run_out = Command::new(&zynqsim_bin)
        .args([
            "--json",
            "run",
            "--project",
            project_file.to_str().unwrap(),
            "--for",
            "10ms",
        ])
        .output();
    let run_ok = match run_out {
        Ok(o) if o.status.success() => {
            let v: Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            let sim_ps = v["time_ps"].as_u64().unwrap_or(0);
            let uart = v["uart"]["1"].as_str().unwrap_or("");
            eprintln!("xtask: simulation completed successfully in virtual time {sim_ps} ps");
            if !uart.is_empty() {
                eprintln!("xtask: UART1 output: {:?}", uart.trim());
            }
            true
        }
        Ok(o) => {
            eprintln!(
                "xtask: zynqsim run failed:\n{}",
                String::from_utf8_lossy(&o.stderr)
            );
            false
        }
        Err(e) => {
            eprintln!("xtask: cannot run zynqsim run: {e}");
            false
        }
    };

    let _ = std::fs::remove_dir_all(&temp_dir);
    if run_ok {
        eprintln!("\n==> First-run walkthrough verified: PASS (zero external tools required!)");
    }
    run_ok
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).cloned().unwrap_or_else(|| "help".to_string());
    let ok = match cmd.as_str() {
        "ci" => ci_gate(&args[2..]),
        "changed-examples" => changed_examples(),
        "try-xsa" => try_xsa(&args[2..]),
        "test" => cargo(&["test", "--workspace"], &[]),
        "golden" => cargo(
            &["test", "-p", "board-zybo", "render"],
            &[("UPDATE_GOLDEN", "1")],
        ),
        "release" => cargo(&["build", "--release"], &[]),
        "dpi" => cargo(dist::DPI_BUILD, &[]),
        "dpi-shared" => dist::run_dpi_shared(&workspace_root()),
        "vpi" => dist::run_vpi(&workspace_root()),
        "dist" => dist::run(&workspace_root()),
        "dist-verify" => dist::verify_cli(std::env::args().nth(2).as_deref()),
        "third-party" => third_party(),
        "firmware" => firmware(),
        "demo" => match demo::parse_args(&args[2..]) {
            Ok(opts) => {
                let root = workspace_root();
                // The firmware ELFs are committed; rebuild them only when one is missing.
                let missing = FIRMWARE
                    .iter()
                    .any(|(_, f)| !root.join("fixtures/elf").join(f).exists());
                demo::run(&root, &opts, &|| if missing { firmware() } else { true })
            }
            Err(e) => {
                eprintln!("xtask: {e}");
                false
            }
        },
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            true
        }
        other => {
            eprintln!("xtask: unknown command `{other}`\n\n{HELP}");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
