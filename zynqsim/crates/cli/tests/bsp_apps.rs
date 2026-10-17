//! Success criterion 2 of the prompt: unmodified Xilinx standalone-BSP applications run on the
//! simulated Zynq-7000 and behave as on hardware at register level.
//!
//! `fixtures/bsp/*.elf` are built by `fixtures/bsp/build_bsp.tcl` (Vitis 2022.1 `xsct`) from
//! Xilinx's own application templates and driver examples, with no source of ours (see
//! `fixtures/bsp/SOURCE.md`). Each one is run with `zynqsim run` on the Xilinx QEMU and its UART1
//! text is checked.
//!
//! Two kinds of expectation, kept apart on purpose:
//!
//! * `PASS`: the application prints its own success line. This is what the hardware prints.
//! * `DEVIATION`: the application does NOT succeed here, and the reason is the QEMU machine,
//!   not the application (docs/conformance/bsp-apps.md lists each cause). The test asserts the
//!   observed failure text so that a change in the model (a peripheral that gets implemented)
//!   fails the test and forces the table and the document to be updated, not silently pass.
//!
//! They need the Xilinx QEMU; without it they print `SKIPPED:` and return.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// One `zynqsim` run is given this much wall time; a QEMU that hangs (seen once in a full
/// gate run) is killed and fails the test instead of blocking the suite forever.
const RUN_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

fn zynqsim(args: &[&str]) -> Output {
    let child = Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("zynqsim starts");
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(RUN_LIMIT) {
        Ok(out) => out.expect("zynqsim runs"),
        Err(_) => {
            // Kill the whole tree (zynqsim started QEMU).
            if cfg!(windows) {
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
            } else {
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
            }
            panic!(
                "`zynqsim {}` did not finish within {RUN_LIMIT:?}",
                args.join(" ")
            );
        }
    }
}

fn qemu_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_slice(&o.stdout).expect("inspect prints JSON");
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bsp_elf(name: &str) -> PathBuf {
    let p = root().join("fixtures/bsp").join(format!("{name}.elf"));
    assert!(p.is_file(), "missing {}", p.display());
    p
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-bsp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What a run produced.
struct Run {
    uart1: String,
    trace_hash: String,
    now_ps: u64,
}

fn run_project(project: &Path, duration: &str) -> Result<Run, String> {
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &project.to_string_lossy(),
        "--for",
        duration,
    ]);
    if o.status.code() != Some(0) {
        return Err(format!(
            "exit {:?}: {}{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ));
    }
    let v: Value = serde_json::from_slice(&o.stdout).map_err(|e| format!("bad JSON: {e}"))?;
    Ok(Run {
        uart1: v["uart"]["1"].as_str().unwrap_or("").replace('\r', ""),
        trace_hash: v["trace_hash"].as_str().unwrap_or("").to_string(),
        now_ps: v["now_ps"].as_u64().ok_or("no now_ps")?,
    })
}

/// A project that runs `elf` on the Zybo Z7-20 PS (no PL), with extra TOML sections (`extra`).
/// `quantum_ns` is the PS/PL synchronisation quantum: there is no PL here, so a long quantum
/// only makes seconds of simulated time cheaper (the result of a PS-only run does not depend on it).
fn write_project(dir: &Path, elf: &Path, extra: &str, quantum_ns: u32) -> PathBuf {
    let mut t = format!(
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"qemu\"\npl = \"demo\"\n\
         [sim]\nquantum_ns = {quantum_ns}\n[boot]\nmode = \"elf\"\n[cpu0]\nelf = \"{}\"\n",
        elf.to_string_lossy().replace('\\', "/")
    );
    t.push_str(extra);
    let p = dir.join("project.toml");
    std::fs::write(&p, t).unwrap();
    p
}

fn example(name: &str) -> PathBuf {
    root().join("examples").join(name).join("project.toml")
}

/// Expected UART1 content of one application.
struct App {
    elf: &'static str,
    /// Simulated time to run: long enough for the application to print its verdict.
    duration: &'static str,
    /// Lines (substrings) that must all be present.
    present: &'static [&'static str],
    /// Substrings that must not appear.
    absent: &'static [&'static str],
    /// Extra project sections (`[storage]`, `[network]`).
    extra: &'static str,
    /// `[sim] quantum_ns` (1000 unless the run takes simulated seconds).
    quantum_ns: u32,
}

const fn app(elf: &'static str, duration: &'static str, present: &'static [&'static str]) -> App {
    App {
        elf,
        duration,
        present,
        absent: &[],
        extra: "",
        quantum_ns: 1000,
    }
}

/// The application runs unmodified and prints its success line.
const PASS: &[App] = &[
    app(
        "bsp_uartps_polled",
        "15ms",
        &["Successfully ran UART Polled Mode Example Test"],
    ),
    app(
        "bsp_uartps_intr",
        "15ms",
        &["Successfully ran UART Interrupt Example Test"],
    ),
    app(
        "bsp_uartps_selftest",
        "15ms",
        &["Successfully ran UART Selftest Example"],
    ),
    app("bsp_scugic", "15ms", &["Successfully ran GIC Example Test"]),
    app(
        "bsp_scugic_lowlevel",
        "15ms",
        &["Successfully ran Low Level GIC Example Test"],
    ),
    app(
        "bsp_scutimer_polled",
        "15ms",
        &["Successfully ran SCU Timer Polled Mode Example Test"],
    ),
    app(
        "bsp_scutimer_intr",
        "15ms",
        &["Successfully ran SCU Timer Interrupt Example Test"],
    ),
    app(
        "bsp_ttcps_tapp",
        "60ms",
        &["Successfully ran ttcps tapp Example"],
    ),
    // The Xilinx example changes the PWM duty after 500 ticks of 100 Hz = 5 s, so it needs
    // simulated seconds; the 100 us quantum keeps that to about 25 s of wall time.
    App {
        quantum_ns: 100_000,
        ..app(
            "bsp_ttcps_intr",
            "7s",
            &["Successfully ran TTC Interrupt Example Test"],
        )
    },
    app(
        "bsp_ttcps_lowlevel",
        "300ms",
        &["Successfully ran TTC Lowlevel Example Test"],
    ),
    app(
        "bsp_dmaps_intr",
        "15ms",
        &["Successfully ran XDMaPs_Example_W_Intr"],
    ),
    app(
        "bsp_qspips_selftest",
        "15ms",
        &["Successfully ran QSPI Selftest Example"],
    ),
    app(
        "bsp_iicps_selftest",
        "15ms",
        &["Successfully ran IIC Self Test Example Test"],
    ),
    // CAN0 (0xE0008000) is mapped since ADR-029 (`dtb::DEFAULT_EDITS`).
    app(
        "bsp_canps_polled",
        "15ms",
        &["Successfully ran CAN Polled Mode Example Test"],
    ),
    // The example addresses sector 204800 (100 MiB): the default 8 MiB card from `sd_dir` is
    // too small and the read-back differs, so the card is made 128 MiB.
    App {
        extra: "[storage]\nsd_dir = \"@SDDIR@\"\nsd_size_mb = 128\nwritable = true\n",
        ..app(
            "bsp_sdps_raw",
            "100ms",
            &["Successfully ran SD Raw Read/ Write Test"],
        )
    },
    // Runs to its own success line, but see DEVIATIONS: the XADC returns zeros.
    app(
        "bsp_xadcps_polled",
        "15ms",
        &["Successfully ran adcps polled printf Example"],
    ),
];

/// The application does not succeed on this machine; the cause is the QEMU model.
const DEVIATIONS: &[App] = &[
    // XADC: every channel reads raw 0 (temperature -273.149 C, rails 0.000 V); hardware reads about
    // 30-60 C and 1.0 V on VCCPINT.
    App {
        absent: &[],
        ..app(
            "bsp_xadcps_polled",
            "15ms",
            &[
                "The Current Temperature is -273.149 Centigrades.",
                "The Current VCCPINT is 0.000 Volts.",
            ],
        )
    },
    // No device-configuration block: CTRL.NIDEN (bit 4) reads back 0 after the write. The FDT
    // machine has no compatible string for the devcfg model (ADR-029).
    app(
        "bsp_devcfg_selftest",
        "15ms",
        &["DevCfg Selftest Example Failed"],
    ),
    // No PS GPIO controller: a pin written 1 reads back 0. The example busy-waits ~100 ms of
    // simulated time between the writes before it can notice.
    app(
        "bsp_gpiops_polled",
        "300ms",
        &["GPIO Polled Mode Example Test Failed"],
    ),
    app(
        "bsp_gpiops_intr",
        "60ms",
        &["GPIO Interrupt Example Test Failed"],
    ),
    // No SPI0/SPI1 controller (the self test reads back the configuration register).
    app(
        "bsp_spips_selftest",
        "15ms",
        &["SPIPS Selftest Example Failed"],
    ),
    // No SCU watchdog (0xF8F00620): the timer never expires and the example spins forever.
    App {
        absent: &["Successfully ran SCU WDT", "Failed"],
        ..app(
            "bsp_scuwdt_polled",
            "15ms",
            &["SCU WDT Polled Mode Example Test"],
        )
    },
];

fn check(app: &App, out: &str, kind: &str) -> Vec<String> {
    let mut bad = Vec::new();
    for p in app.present {
        if !out.contains(p) {
            bad.push(format!("{kind} {}: missing {p:?}", app.elf));
        }
    }
    for a in app.absent {
        if out.contains(a) {
            bad.push(format!("{kind} {}: unexpected {a:?}", app.elf));
        }
    }
    bad
}

fn run_app(app: &App, sd_dir: &Path, tag: &str) -> Result<String, String> {
    let dir = temp(&format!("{tag}-{}", app.elf));
    let extra = app
        .extra
        .replace("@SDDIR@", &sd_dir.to_string_lossy().replace('\\', "/"));
    let project = write_project(&dir, &bsp_elf(app.elf), &extra, app.quantum_ns);
    let r = run_project(&project, app.duration);
    let _ = std::fs::remove_dir_all(&dir);
    r.map(|r| r.uart1).map_err(|e| format!("{}: {e}", app.elf))
}

/// Xilinx's "Hello World" template, unmodified, through the example project: the UART1 text is
/// the template's own, and two runs give one trace digest.
#[test]
fn bsp_hello_world_runs_unmodified_and_deterministically() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let project = example("bsp-hello");
    let a = run_project(&project, "15ms").expect("first run");
    let b = run_project(&project, "15ms").expect("second run");
    assert!(a.uart1.contains("Hello World\n"), "UART1: {:?}", a.uart1);
    assert!(
        a.uart1.contains("Successfully ran Hello World application"),
        "UART1: {:?}",
        a.uart1
    );
    assert_eq!(a.now_ps, 15_000_000_000 + 1_000_000, "ran to the target");
    assert!(!a.trace_hash.is_empty());
    assert_eq!((&a.trace_hash, a.now_ps), (&b.trace_hash, b.now_ps));
    assert_eq!(a.uart1, b.uart1);
}

/// Xilinx's "Peripheral Tests" template, unmodified (testperiph.c generated by Vitis from the
/// drivers' own test scripts). The PASSED / FAILED lines are the application's.
#[test]
fn bsp_peripheral_tests_report_what_the_model_implements() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let r = run_project(&example("bsp-peripherals"), "15ms").expect("run");
    let out = r.uart1;
    // Implemented and passing, as on hardware.
    for line in [
        "---Entering main---",
        "ScuGicSelfTestExample PASSED",
        "ScuGic Interrupt Setup PASSED",
        "XDmaPs_Example_W_Intr PASSED",
        "IicPsSelfTestExample PASSED",
        "QspiPsSelfTestExample PASSED",
        "ScuTimerPolledExample PASSED",
        "ScuTimerIntrExample PASSED",
        // CAN0 is mapped (ADR-029): polled and interrupt tests pass.
        "CanPsPolledExample PASSED",
        "CanPsIntrExample PASSED",
    ] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
    // DEVIATION: peripherals this QEMU machine does not have (docs/conformance/bsp-apps.md).
    assert!(
        out.contains("DcfgSelfTestExample FAILED"),
        "missing recorded deviation \"DcfgSelfTestExample FAILED\" in:\n{out}"
    );
    // DEVIATION: the SCU watchdog does not exist, so its interrupt test never returns.
    assert!(
        out.contains("Running Interrupt Test  for ps7_scuwdt_0"),
        "{out}"
    );
    assert!(!out.contains("ScuWdtIntrExample"), "{out}");
    assert!(!out.contains("---Exiting main---"), "{out}");
}

/// One Xilinx driver example per application, each built as it is (its own `main`).
#[test]
fn bsp_driver_examples_match_the_recorded_results() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let sd_dir = root().join("examples/sd-demo/sdcard");
    let jobs: Vec<(&App, &str)> = PASS
        .iter()
        .map(|a| (a, "PASS"))
        .chain(DEVIATIONS.iter().map(|a| (a, "DEVIATION")))
        .collect();
    let next = AtomicUsize::new(0);
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..3)
            .map(|w| {
                let (next, jobs, sd_dir) = (&next, &jobs, &sd_dir);
                s.spawn(move || {
                    let mut bad = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some((app, kind)) = jobs.get(i) else {
                            break bad;
                        };
                        match run_app(app, sd_dir, &format!("w{w}")) {
                            Ok(out) => {
                                let f = check(app, &out, kind);
                                if f.is_empty() {
                                    eprintln!("{kind:<9} {}: ok", app.elf);
                                }
                                for m in f {
                                    bad.push(format!("{m}\nUART1:\n{out}"));
                                }
                            }
                            Err(e) => bad.push(e),
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker runs"))
            .collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The GEM driver's own DMA/interrupt example (`xemacps_example_intr_dma.c`, PHY loopback). It
/// prints its success line only after about 40 simulated seconds (the example sleeps for
/// seconds between steps, and the BSP's `sleep()` counts the A9 global timer, which is 3.3x
/// slower here than on silicon, see docs/conformance/bsp-apps.md), about 13 minutes of wall
/// time even with a 100 us quantum. So it is `#[ignore]`d: `cargo test -p zynqsim --test
/// bsp_apps -- --ignored`.
#[test]
#[ignore = "about 13 minutes of wall time"]
fn bsp_emacps_intr_dma_prints_its_success_line() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let sd = root().join("examples/sd-demo/sdcard");
    let a = App {
        extra: "[network]\nmac = \"00:0a:35:00:1e:53\"\nsubnet = \"10.0.2.0/24\"\ngateway = \"10.0.2.2\"\nrestrict = true\n",
        quantum_ns: 100_000,
        ..app(
            "bsp_emacps_intr_dma",
            "40s",
            &[
                "Entering into main()",
                "Successfully ran Emacps intr dma Example",
            ],
        )
    };
    let out = run_app(&a, &sd, "emac").expect("run");
    let bad = check(&a, &out, "PASS");
    assert!(bad.is_empty(), "{}\nUART1:\n{out}", bad.join("\n"));
}

/// Every committed BSP ELF is small (the prompt-level budget: < 300 KB) and has a build entry in
/// `SOURCE.md`, so no unexplained binary sits in `fixtures/bsp`.
#[test]
fn every_bsp_elf_is_small_and_documented() {
    let dir = root().join("fixtures/bsp");
    let source = std::fs::read_to_string(dir.join("SOURCE.md")).expect("SOURCE.md");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "elf") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let len = std::fs::metadata(&p).unwrap().len();
            assert!(len < 300 * 1024, "{name}.elf is {len} bytes");
            assert!(source.contains(&name), "{name} is not in SOURCE.md");
            n += 1;
        }
    }
    assert!(n >= 20, "only {n} ELFs");
}
