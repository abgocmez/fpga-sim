//! `cargo xtask demo`: a curated tour of the headless checks, run through the real
//! `zynqsim` binary (and `zynqsim-mcp` over stdio). Every step states the result it
//! expects and CHECKS it: UART text, `--json` fields, exit codes, artifact files.
//! Steps whose tool is not installed are SKIPPED with the reason, never FAILed.
//! Argument parsing, the tour table and the checks are pure and unit-tested here.
//! The tour's types are internal plumbing of this one command, described in
//! docs/DEMO.md rather than per field, hence the allowance below.
#![allow(missing_docs)]

use serde_json::{Value, json};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Options of `cargo xtask demo`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Opts {
    /// The 5-minute subset (`quick` steps only).
    pub quick: bool,
    /// Also take the GUI screenshots (section k).
    pub gui: bool,
    /// Run only these steps (by name).
    pub only: Option<Vec<String>>,
}

const USAGE: &str = "cargo xtask demo [--quick] [--only name,...] [--gui]";

/// Parses the arguments that follow `demo`.
pub fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--quick" => o.quick = true,
            "--gui" => o.gui = true,
            "--only" => {
                i += 1;
                let list = args
                    .get(i)
                    .ok_or("--only needs a comma-separated list of step names")?;
                o.only = Some(parse_only(list)?);
            }
            other => match other.strip_prefix("--only=") {
                Some(list) => o.only = Some(parse_only(list)?),
                None => return Err(format!("unknown option `{other}`; usage: {USAGE}")),
            },
        }
        i += 1;
    }
    if o.quick && o.only.is_some() {
        return Err("--quick and --only cannot be combined".into());
    }
    Ok(o)
}

fn parse_only(list: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = list
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect();
    if names.is_empty() {
        Err("--only needs at least one step name".into())
    } else {
        Ok(names)
    }
}

/// What a step needs installed besides the built-in engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Qemu,
    Xsim,
    Verilator,
    Icarus,
}

impl Need {
    /// The `name` of the backend in `zynqsim inspect --json`.
    pub fn backend(self) -> &'static str {
        match self {
            Need::Qemu => "qemu",
            Need::Xsim => "xsim",
            Need::Verilator => "verilator",
            Need::Icarus => "icarus",
        }
    }

    /// Why a step is skipped when the tool is missing.
    pub fn missing_reason(self) -> &'static str {
        match self {
            Need::Qemu => "Xilinx QEMU not found (Vitis install, or ZYNQSIM_QEMU)",
            Need::Xsim => "Vivado XSim not found (Vivado install, or ZYNQSIM_VIVADO)",
            Need::Verilator => "Verilator not found",
            Need::Icarus => "Icarus Verilog not found (iverilog and vvp)",
        }
    }
}

/// One expectation on a run's result.
#[derive(Debug, Clone, Copy)]
pub enum Check {
    /// UART1 text contains this.
    Uart1(&'static str),
    /// The `--json` field at this JSON pointer equals this JSON literal.
    Json(&'static str, &'static str),
    /// The `--json` string field at this JSON pointer contains this text.
    JsonHas(&'static str, &'static str),
    /// Command stdout contains this text (for `Kind::Cmd`).
    Stdout(&'static str),
}

/// Magic-number check on an artifact file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Magic {
    Png,
    Wav,
    Pcap,
}

impl Magic {
    fn ok(self, bytes: &[u8]) -> bool {
        match self {
            Magic::Png => bytes.starts_with(&[0x89, b'P', b'N', b'G']),
            Magic::Wav => bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
            Magic::Pcap => {
                bytes.starts_with(&[0xd4, 0xc3, 0xb2, 0xa1])
                    || bytes.starts_with(&[0xa1, 0xb2, 0xc3, 0xd4])
            }
        }
    }
}

/// How a step is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `zynqsim --json run --project P --for D ARGS` with the checks.
    Run,
    /// `zynqsim ARGS` (any other subcommand), checked on stdout.
    Cmd,
    /// The same run three times: the trace hash must be identical.
    Determinism,
    /// Drives `zynqsim-mcp` over stdio with a scripted conversation.
    Mcp(McpScript),
    /// `zynqsim-gui ARGS` writes a screenshot and exits.
    Gui,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScript {
    Tour,
    Debug,
}

/// One row of the tour.
#[derive(Debug, Clone)]
pub struct Step {
    pub section: char,
    pub name: &'static str,
    pub kind: Kind,
    /// Project file, relative to the `zynqsim/` root (may contain `{out}`).
    pub project: &'static str,
    pub duration: &'static str,
    /// Extra arguments. `{out}` is replaced by the `demo-out` folder.
    pub args: &'static [&'static str],
    pub needs: &'static [Need],
    pub checks: &'static [Check],
    /// Files the step must write, with the magic number they must start with.
    pub outputs: &'static [(&'static str, Magic)],
    /// Folder under `demo-out` removed before the step runs.
    pub clean: &'static str,
    /// Steps with the same group must produce the same trace hash.
    pub hash_group: Option<&'static str>,
    /// Part of `--quick`.
    pub quick: bool,
}

const fn step(section: char, name: &'static str, kind: Kind) -> Step {
    Step {
        section,
        name,
        kind,
        project: "",
        duration: "",
        args: &[],
        needs: &[],
        checks: &[],
        outputs: &[],
        clean: "",
        hash_group: None,
        quick: false,
    }
}

/// Section titles, in order.
pub const SECTIONS: &[(char, &str)] = &[
    ('a', "PS only (no PL)"),
    ('b', "PL through QEMU and Vivado XSim (RTL)"),
    ('c', "Other RTL backends and native RTL"),
    ('d', "Media: HDMI and audio"),
    ('e', "Network and storage"),
    ('f', "Vendor IP (no RTL)"),
    ('g', "Debugger"),
    ('h', "Vivado hand-off"),
    ('i', "Determinism"),
    ('j', "MCP server"),
    ('k', "GUI screenshots (--gui)"),
];

/// Examples that cannot run, with the reason from docs/STATUS.md and the example header.
pub const KNOWN_NOT_WORKING: &[(&str, &str)] = &[
    (
        "native-math",
        "the Xilinx boot code reads unmapped 0xF800D010 before main: a data abort, then the abort vector loops (ADR-035, STATUS.md)",
    ),
    (
        "bsp-peripherals (CAN and devcfg self tests, SCU watchdog)",
        "FAILED on this QEMU machine: CAN and device configuration are not in the model; the SCU watchdog interrupt test never returns (docs/conformance/bsp-apps.md). Not in the tour",
    ),
    (
        "board-demo across processes",
        "its shell input over the UART TCP path arrives at wall-clock dependent virtual times: trace hash differs between runs (STATUS.md FINDING). Not checked for determinism",
    ),
    (
        "math-debug",
        "debugger timing under QEMU is not reproducible between runs; only the guest state at each stop is. Driven by hand through zynqsim-mcp, not in the tour",
    ),
    (
        "unmodelled IP in a Vivado hand-off",
        "an [xsa.unmodelled] entry stops the run with a diagnostic by design (validation error), so it is not a demo result",
    ),
    (
        "SPI and GPIO conformance on the default model",
        "SPI is not modelled by default and the PS GPIO is not mapped by QEMU: their checks are N/A, not PASS (docs/conformance/iic-spi-xadc.md)",
    ),
];

const MATH_CHECK: Check = Check::Uart1("RESULT: ALL PASS");

/// The tour, in run order.
fn ps(
    name: &'static str,
    project: &'static str,
    duration: &'static str,
    checks: &'static [Check],
    needs: &'static [Need],
) -> Step {
    Step {
        project,
        duration,
        checks,
        needs,
        ..step('a', name, Kind::Run)
    }
}

/// Steps of sections a, g, i and j, and two named steps, are in `--quick`.
fn quick_step(s: &Step) -> bool {
    matches!(s.section, 'a' | 'g' | 'i' | 'j') || matches!(s.name, "math-qemu" | "math-verilator")
}

pub fn tour() -> Vec<Step> {
    let mut steps = tour_steps();
    for s in &mut steps {
        s.quick = quick_step(s);
    }
    steps
}

fn tour_steps() -> Vec<Step> {
    vec![
        // (a) PS only
        ps(
            "rust-hello",
            "examples/rust-hello/project.toml",
            "15ms",
            &[Check::Uart1("sum(1..=100) = 5050"), MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "native-hello",
            "examples/native-hello/project.toml",
            "20ms",
            &[Check::Uart1("sum(1..=100) = 5050"), MATH_CHECK],
            &[],
        ),
        ps(
            "native-thumb",
            "examples/native-thumb/project.toml",
            "20ms",
            &[Check::Uart1("ok: factorial(10)")],
            &[],
        ),
        ps(
            "dual-core-native",
            "examples/dual-core-native/project.toml",
            "20ms",
            &[
                Check::Uart1("counter 1000 (expected 1000): PASS"),
                Check::Uart1("SGI 1 delivered to core 1"),
            ],
            &[],
        ),
        ps(
            "dual-core-qemu",
            "examples/dual-core-qemu/project.toml",
            "80ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-scutimer",
            "examples/conf-scutimer/project.toml",
            "200ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-ttc",
            "examples/conf-ttc/project.toml",
            "50ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "bsp-hello",
            "examples/bsp-hello/project.toml",
            "15ms",
            &[Check::Uart1("Successfully ran Hello World application")],
            &[Need::Qemu],
        ),
        ps(
            "timer-rate",
            "examples/timer-rate/project.toml",
            "8ms",
            &[Check::Uart1("DONE_GT ticks=1000000")],
            &[Need::Qemu],
        ),
        ps(
            "conf-iicps",
            "examples/conf-iicps/project.toml",
            "50ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-spips",
            "examples/conf-spips/project.toml",
            "50ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-xadcps",
            "examples/conf-xadcps/project.toml",
            "50ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-gpiops",
            "examples/conf-gpiops/project.toml",
            "20ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        ps(
            "conf-dmaps",
            "examples/conf-dmaps/project.toml",
            "50ms",
            &[MATH_CHECK],
            &[Need::Qemu],
        ),
        // (b) PL through QEMU and XSim
        Step {
            hash_group: Some("math"),
            ..ps(
                "math-qemu",
                "examples/math-qemu/project.toml",
                "2ms",
                &[MATH_CHECK, Check::Uart1("COSIM_DONE")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'b',
            ..ps(
                "dma-regression",
                "examples/dma-regression/project.toml",
                "4ms",
                &[MATH_CHECK, Check::Uart1("pipeline (6 jobs")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            ..ps(
                "irq-demo",
                "examples/irq-demo/project.toml",
                "4ms",
                &[MATH_CHECK, Check::Uart1("events=3 handled=3 spurious=0")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &[
                "--at",
                "3ms=uart1:led 5\\r",
                "--at",
                "6ms=sw1:on",
                "--at",
                "7ms=uart1:status\\r",
            ],
            ..ps(
                "board-demo",
                "examples/board-demo/project.toml",
                "15ms",
                &[
                    Check::Uart1("LED=5 SW=2 BTN=0"),
                    Check::Json("/leds/0/duty", "255"),
                    Check::Json("/leds/2/duty", "255"),
                ],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &["--device-out", "JA={out}/oled.png"],
            outputs: &[("oled.png", Magic::Png)],
            ..ps(
                "oled-demo",
                "examples/oled-demo/project.toml",
                "3ms",
                &[MATH_CHECK, Check::Uart1("sent 512 data bytes")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            ..ps(
                "rgb-demo",
                "examples/rgb-demo/project.toml",
                "2ms",
                &[MATH_CHECK, Check::Json("/rgb", "[[64,128,254],[254,0,32]]")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &["--device-set", "JA.temperature=-12.25"],
            ..ps(
                "tmp2-demo",
                "examples/tmp2-demo/project.toml",
                "8ms",
                &[Check::Uart1("T16 raw=0xF9E0 mC=-12250")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &["--device-set", "JA.light=20"],
            ..ps(
                "als-demo",
                "examples/als-demo/project.toml",
                "4ms",
                &[Check::Uart1("LIGHT frame=0x0140 code=20")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &["--device-set", "JA.accel_z=-1"],
            ..ps(
                "acl-demo",
                "examples/acl-demo/project.toml",
                "4ms",
                &[Check::Uart1("ACC x=500 y=-250 z=-1000 mg")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            args: &[
                "--device-set",
                "3ms@JB.btn0=1",
                "--device-set",
                "3ms@JB.sw2=1",
            ],
            ..ps(
                "pmod-simple-demo",
                "examples/pmod-simple-demo/project.toml",
                "40ms",
                &[MATH_CHECK, Check::Uart1("BTN 0x41")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'b',
            ..ps(
                "pcam-demo",
                "examples/pcam-demo/project.toml",
                "40ms",
                &[MATH_CHECK, Check::Uart1("CAP squares")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        // (c) other RTL backends and native RTL
        Step {
            section: 'c',
            hash_group: Some("math"),
            ..ps(
                "math-verilator",
                "examples/math-verilator/project.toml",
                "2ms",
                &[MATH_CHECK],
                &[Need::Qemu, Need::Verilator],
            )
        },
        Step {
            section: 'c',
            hash_group: Some("math"),
            ..ps(
                "math-icarus",
                "examples/math-icarus/project.toml",
                "2ms",
                &[MATH_CHECK],
                &[Need::Qemu, Need::Icarus],
            )
        },
        Step {
            section: 'c',
            ..ps(
                "native-gpio-timer-uart",
                "examples/native-gpio-timer-uart/project.toml",
                "60ms",
                &[MATH_CHECK, Check::Uart1("timer ticks=3")],
                &[],
            )
        },
        Step {
            section: 'c',
            kind: Kind::Cmd,
            args: &["rtl-check", "fixtures/rtl/math/my_math_ip.v"],
            checks: &[Check::Stdout("no diagnostics")],
            quick: false,
            ..step('c', "rtl-check", Kind::Cmd)
        },
        // (d) media
        Step {
            section: 'd',
            args: &["--video-out", "{out}/hdmi.png"],
            outputs: &[("hdmi.png", Magic::Png)],
            ..ps(
                "hdmi-demo",
                "examples/hdmi-demo/project.toml",
                "40ms",
                &[MATH_CHECK, Check::Uart1("CAPTURE frames=1")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'd',
            args: &["--audio-out", "{out}/tone.wav"],
            outputs: &[("tone.wav", Magic::Wav)],
            ..ps(
                "audio-demo",
                "examples/audio-demo/project.toml",
                "160ms",
                &[MATH_CHECK, Check::Uart1("LOOPBACK frames=1000")],
                &[Need::Qemu, Need::Xsim],
            )
        },
        // (e) network and storage
        Step {
            section: 'e',
            args: &["--pcap", "{out}/net.pcap"],
            outputs: &[("net.pcap", Magic::Pcap)],
            ..ps(
                "net-demo",
                "examples/net-demo/project.toml",
                "10ms",
                &[MATH_CHECK, Check::Uart1("4/4 replies")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'e',
            ..ps(
                "sd-demo",
                "examples/sd-demo/project.toml",
                "60ms",
                &[MATH_CHECK, Check::Uart1("FILE HELLO.TXT")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'e',
            ..ps(
                "qspi-demo",
                "examples/qspi-demo/project.toml",
                "60ms",
                &[MATH_CHECK],
                &[Need::Qemu],
            )
        },
        // (f) vendor IP
        Step {
            section: 'f',
            ..ps(
                "gpio-timer-uart",
                "examples/gpio-timer-uart/project.toml",
                "6ms",
                &[MATH_CHECK, Check::Uart1("timer ticks=3")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'f',
            ..ps(
                "dma-loopback",
                "examples/dma-loopback/project.toml",
                "4ms",
                &[
                    MATH_CHECK,
                    Check::Uart1("copied 1024 bytes, 0 word(s) differ"),
                ],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'f',
            ..ps(
                "dma-sg",
                "examples/dma-sg/project.toml",
                "3ms",
                &[MATH_CHECK, Check::Uart1("rx status: 0x88000030 0x84000010")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'f',
            ..ps(
                "iic-bram-demo",
                "examples/iic-bram-demo/project.toml",
                "8ms",
                &[MATH_CHECK, Check::Uart1("ID 0xCB")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'f',
            ..ps(
                "intc-demo",
                "examples/intc-demo/project.toml",
                "3ms",
                &[MATH_CHECK, Check::Uart1("timer ticks=3 (via input 1)")],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'f',
            args: &["--at", "3ms=btn0:down", "--at", "4ms=btn0:up"],
            ..ps(
                "timer-modes",
                "examples/timer-modes/project.toml",
                "20ms",
                &[MATH_CHECK, Check::Uart1("periodic_ok=1")],
                &[Need::Qemu],
            )
        },
        // (g) debugger
        Step {
            section: 'g',
            args: &["--break", "main.rs:19"],
            checks: &[
                Check::Json("/debug/stopped", "true"),
                Check::Json("/debug/source/line", "19"),
                Check::JsonHas("/debug/pc_symbol", "collatz_steps"),
            ],
            ..ps(
                "rust-debug",
                "examples/rust-debug/project.toml",
                "1ms",
                &[],
                &[Need::Qemu],
            )
        },
        Step {
            section: 'g',
            kind: Kind::Mcp(McpScript::Debug),
            project: "examples/rust-debug/project.toml",
            needs: &[Need::Qemu],
            ..step('g', "rust-debug-regs", Kind::Mcp(McpScript::Debug))
        },
        // (h) Vivado hand-off
        Step {
            section: 'h',
            hash_group: Some("handoff"),
            ..ps(
                "vivado-handoff",
                "examples/vivado-handoff/project.toml",
                "15ms",
                &[MATH_CHECK],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'h',
            kind: Kind::Cmd,
            args: &[
                "init",
                "--xsa",
                "fixtures/handoff/design.xsa",
                "--rtl",
                "fixtures/handoff/rtl/my_math_ip.v",
                "--boot",
                "fixtures/handoff/BOOT.BIN",
                "--out",
                "{out}/handoff-project",
            ],
            clean: "handoff-project",
            checks: &[Check::Stdout("created")],
            ..step('h', "handoff-init", Kind::Cmd)
        },
        Step {
            section: 'h',
            hash_group: Some("handoff"),
            project: "{out}/handoff-project/project.toml",
            ..ps(
                "handoff-run",
                "",
                "15ms",
                &[MATH_CHECK],
                &[Need::Qemu, Need::Xsim],
            )
        },
        Step {
            section: 'h',
            kind: Kind::Cmd,
            args: &["boot-info", "fixtures/handoff/BOOT.BIN"],
            checks: &[Check::Stdout("runnable: yes")],
            ..step('h', "boot-info", Kind::Cmd)
        },
        // (i) determinism
        Step {
            section: 'i',
            kind: Kind::Determinism,
            project: "examples/rust-hello/project.toml",
            duration: "15ms",
            needs: &[Need::Qemu],
            ..step('i', "determinism", Kind::Determinism)
        },
        // (j) MCP
        Step {
            section: 'j',
            kind: Kind::Mcp(McpScript::Tour),
            project: "examples/board-demo/project.toml",
            needs: &[Need::Qemu, Need::Xsim],
            ..step('j', "mcp-tour", Kind::Mcp(McpScript::Tour))
        },
    ]
}

fn gui(
    name: &'static str,
    project: &'static str,
    args: &'static [&'static str],
    shot: &'static [(&'static str, Magic)],
) -> Step {
    Step {
        project,
        args,
        outputs: shot,
        ..step('k', name, Kind::Gui)
    }
}

/// The GUI screenshots of section k (only with `--gui`).
pub fn gui_steps() -> Vec<Step> {
    vec![
        gui(
            "gui-board",
            "examples/board-demo/project.toml",
            &[
                "--run-ms",
                "15",
                "--view3d",
                "--screenshot",
                "{out}/gui/board.png",
                "--exit-after-ms",
                "1500",
            ],
            &[("gui/board.png", Magic::Png)],
        ),
        gui(
            "gui-rust-debug",
            "examples/rust-debug/project.toml",
            &[
                "--debugger",
                "--screenshot",
                "{out}/gui/debugger.png",
                "--exit-after-ms",
                "1500",
            ],
            &[("gui/debugger.png", Magic::Png)],
        ),
        gui(
            "gui-oled",
            "examples/oled-demo/project.toml",
            &[
                "--run-ms",
                "3",
                "--screenshot",
                "{out}/gui/oled.png",
                "--exit-after-ms",
                "1500",
            ],
            &[("gui/oled.png", Magic::Png)],
        ),
        gui(
            "gui-net",
            "examples/net-demo/project.toml",
            &[
                "--run-ms",
                "10",
                "--screenshot",
                "{out}/gui/net.png",
                "--exit-after-ms",
                "1500",
            ],
            &[("gui/net.png", Magic::Png)],
        ),
    ]
}

/// Outcome of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Res {
    Pass,
    Fail(String),
    Skip(String),
}

impl Res {
    pub fn label(&self) -> &'static str {
        match self {
            Res::Pass => "PASS",
            Res::Fail(_) => "FAIL",
            Res::Skip(_) => "SKIPPED",
        }
    }
}

/// Evaluates the checks of a `--json` run result. Returns the failures.
pub fn eval_run(v: &Value, checks: &[Check]) -> Vec<String> {
    let mut fails = Vec::new();
    for c in checks {
        match *c {
            Check::Uart1(text) => {
                let uart = v.pointer("/uart/1").and_then(Value::as_str).unwrap_or("");
                if !uart.contains(text) {
                    fails.push(format!("UART1 does not contain `{text}`"));
                }
            }
            Check::Json(ptr, lit) => {
                let want: Value = serde_json::from_str(lit).expect("tour literal is JSON");
                if v.pointer(ptr) != Some(&want) {
                    fails.push(format!(
                        "{ptr} is {} (expected {lit})",
                        v.pointer(ptr)
                            .map_or("missing".to_string(), Value::to_string)
                    ));
                }
            }
            Check::JsonHas(ptr, text) => {
                let s = v.pointer(ptr).and_then(Value::as_str).unwrap_or("");
                if !s.contains(text) {
                    fails.push(format!("{ptr} does not contain `{text}`"));
                }
            }
            Check::Stdout(_) => {}
        }
    }
    fails
}

/// Substitutes `{out}` in an argument.
pub fn subst(arg: &str, out: &Path) -> String {
    arg.replace("{out}", &out.display().to_string().replace('\\', "/"))
}

/// Returns the tour steps selected by the options. Unknown `--only` names are an error.
pub fn select(steps: Vec<Step>, opts: &Opts) -> Result<Vec<Step>, String> {
    if let Some(names) = &opts.only {
        let known: Vec<&str> = steps.iter().map(|s| s.name).collect();
        if let Some(bad) = names.iter().find(|n| !known.contains(&n.as_str())) {
            return Err(format!("unknown step `{bad}`; steps: {}", known.join(", ")));
        }
        return Ok(steps
            .into_iter()
            .filter(|s| names.iter().any(|n| n == s.name))
            .collect());
    }
    Ok(steps
        .into_iter()
        .filter(|s| !opts.quick || s.quick)
        .collect())
}

/// Parses `zynqsim inspect --json` backends into (name, found) pairs.
pub fn backend_found(inspect: &Value) -> Vec<(String, bool)> {
    inspect["backends"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    Some((
                        b["name"].as_str()?.to_string(),
                        b["found"].as_bool().unwrap_or(false),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The row of the results table.
#[derive(Debug, Clone)]
pub struct Row {
    pub section: char,
    pub name: String,
    pub example: String,
    pub res: Res,
    pub wall: Duration,
    pub artifact: String,
}

/// Renders the results table (plain text, also used for `demo-out/summary.txt`).
pub fn table(rows: &[Row], total: Duration) -> String {
    let mut s = String::new();
    let w_name = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let w_ex = rows
        .iter()
        .map(|r| r.example.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let _ = writeln!(
        s,
        "{:<3} {:<w_name$} {:<w_ex$} {:<8} {:>8}  artifact",
        "sec", "step", "example", "result", "wall"
    );
    for r in rows {
        let result = match &r.res {
            Res::Skip(why) => format!("SKIPPED({why})"),
            other => other.label().to_string(),
        };
        let _ = writeln!(
            s,
            "{:<3} {:<w_name$} {:<w_ex$} {:<8} {:>8}  {}",
            r.section,
            r.name,
            r.example,
            result,
            fmt_wall(r.wall),
            r.artifact
        );
    }
    let count = |l: &str| rows.iter().filter(|r| r.res.label() == l).count();
    let _ = writeln!(
        s,
        "total: {} PASS, {} FAIL, {} SKIPPED in {}",
        count("PASS"),
        count("FAIL"),
        count("SKIPPED"),
        fmt_wall(total)
    );
    s
}

fn fmt_wall(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        format!("{}m{:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    }
}

/// Checks the trace hashes of a hash group: all equal.
pub fn group_verdict(hashes: &[(String, String)]) -> Result<String, String> {
    let first = hashes.first().map(|(_, h)| h.clone()).unwrap_or_default();
    let line = hashes
        .iter()
        .map(|(n, h)| format!("{n}={h}"))
        .collect::<Vec<_>>()
        .join("  ");
    if !first.is_empty() && hashes.iter().all(|(_, h)| *h == first) {
        Ok(line)
    } else {
        Err(line)
    }
}

/// The MCP conversation of section j (or the debugger one).
fn mcp_script(script: McpScript, root: &Path) -> Vec<(u64, &'static str, Value)> {
    let project = root.join("examples/board-demo/project.toml");
    match script {
        McpScript::Tour => vec![
            (
                1,
                "initialize",
                json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "xtask-demo", "version": "0"}}),
            ),
            (2, "tools/list", json!({})),
            (
                3,
                "tools/call",
                json!({"name": "project_open", "arguments": {"path": project.display().to_string()}}),
            ),
            (
                4,
                "tools/call",
                json!({"name": "sim_run", "arguments": {"duration": "5ms"}}),
            ),
            (
                5,
                "tools/call",
                json!({"name": "uart_read", "arguments": {"port": 1}}),
            ),
            (
                6,
                "tools/call",
                json!({"name": "board_get_leds", "arguments": {}}),
            ),
            (
                7,
                "tools/call",
                json!({"name": "waveform_get", "arguments": {"signals": ["board.led*"]}}),
            ),
        ],
        McpScript::Debug => {
            let project = root.join("examples/rust-debug/project.toml");
            vec![
                (
                    1,
                    "initialize",
                    json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "xtask-demo", "version": "0"}}),
                ),
                (
                    2,
                    "tools/call",
                    json!({"name": "project_open", "arguments": {"path": project.display().to_string()}}),
                ),
                (
                    3,
                    "tools/call",
                    json!({"name": "sim_run", "arguments": {"duration": "1ms"}}),
                ),
                (
                    4,
                    "tools/call",
                    json!({"name": "cpu_regs", "arguments": {}}),
                ),
            ]
        }
    }
}

/// Checks one MCP reply. Returns a detail string or the failure.
fn mcp_check(script: McpScript, id: u64, reply: &Value) -> Result<String, String> {
    let result = reply
        .get("result")
        .ok_or_else(|| format!("no result: {reply}"))?;
    if id == 1 {
        return Ok(format!(
            "server {}",
            result
                .pointer("/serverInfo/name")
                .and_then(Value::as_str)
                .unwrap_or("?")
        ));
    }
    if id == 2 && script == McpScript::Tour {
        let n = result["tools"].as_array().map_or(0, Vec::len);
        return if n >= 20 {
            Ok(format!("{n} tools"))
        } else {
            Err(format!("only {n} tools"))
        };
    }
    if result["isError"].as_bool() == Some(true) {
        return Err(format!("tool error: {}", result["content"][0]["text"]));
    }
    // Tool results are JSON text inside content[0].text.
    let text = result["content"][0]["text"].as_str().unwrap_or("{}");
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    match (script, id) {
        (McpScript::Tour, 3) => Ok("project opened".into()),
        (McpScript::Tour, 4) => {
            let ps = body["now_ps"].as_u64().unwrap_or(0);
            if ps > 0 {
                Ok(format!("now_ps={ps}"))
            } else {
                Err(format!("sim_run did not advance: {text}"))
            }
        }
        (McpScript::Tour, 5) => {
            let uart = body["text"].as_str().unwrap_or("");
            if uart.contains("virtual Zybo") {
                Ok("UART1 has the shell banner".into())
            } else {
                Err(format!("UART1 text: {uart:?}"))
            }
        }
        (McpScript::Tour, 6) => {
            let n = body["leds"].as_array().map_or(0, Vec::len);
            if n == 4 {
                Ok("4 LEDs".into())
            } else {
                Err(format!("{n} LEDs"))
            }
        }
        (McpScript::Tour, 7) => {
            if body.get("changes").is_some() || body.get("signals").is_some() {
                Ok("waveform window returned".into())
            } else {
                Err(format!("no waveform data: {text}"))
            }
        }
        (McpScript::Debug, 2) => Ok("project opened".into()),
        (McpScript::Debug, 3) => Ok("ran to the breakpoint".into()),
        (McpScript::Debug, 4) => {
            let s = text.to_string();
            if s.contains("pc") {
                Ok(format!("registers: {}", compact(&s)))
            } else {
                Err(format!("no registers: {s}"))
            }
        }
        _ => Ok(String::new()),
    }
}

fn compact(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() > 160 {
        format!("{}...", &flat[..160])
    } else {
        flat
    }
}

/// Everything the runner needs from the environment.
pub struct Ctx {
    pub root: PathBuf,
    pub out: PathBuf,
    pub zynqsim: PathBuf,
    pub gui: PathBuf,
    pub mcp: PathBuf,
    /// Backends found by `zynqsim inspect --json`.
    pub found: Vec<(String, bool)>,
}

impl Ctx {
    fn has(&self, need: Need) -> bool {
        self.found.iter().any(|(n, f)| n == need.backend() && *f)
    }
}

fn run_zynqsim(ctx: &Ctx, args: &[String]) -> Result<(bool, String, String), String> {
    let out = Command::new(&ctx.zynqsim)
        .args(args)
        .current_dir(&ctx.root)
        .output()
        .map_err(|e| format!("cannot run zynqsim: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Runs one step. Returns the result, the artifact label and the trace hash, if any.
fn run_step(ctx: &Ctx, step: &Step) -> (Res, String, Option<String>) {
    for need in step.needs {
        if !ctx.has(*need) {
            return (Res::Skip(need.missing_reason().into()), String::new(), None);
        }
    }
    if !step.clean.is_empty() {
        let _ = std::fs::remove_dir_all(ctx.out.join(step.clean));
    }
    let artifact_list: Vec<String> = step
        .outputs
        .iter()
        .map(|(f, _)| ctx.out.join(f).display().to_string())
        .collect();
    let artifact = artifact_list.join(", ");
    let _ = std::fs::create_dir_all(ctx.out.join("gui"));
    let args_out: Vec<String> = step.args.iter().map(|a| subst(a, &ctx.out)).collect();
    match step.kind {
        Kind::Run => {
            let project = subst(step.project, &ctx.out);
            let mut args = vec![
                "--json".to_string(),
                "run".into(),
                "--project".into(),
                project,
                "--for".into(),
                step.duration.into(),
            ];
            args.extend(args_out);
            let (ok, stdout, stderr) = match run_zynqsim(ctx, &args) {
                Ok(t) => t,
                Err(e) => return (Res::Fail(e), artifact, None),
            };
            if !ok {
                return (
                    Res::Fail(format!("exit code != 0: {}", last_line(&stderr))),
                    artifact,
                    None,
                );
            }
            let v: Value = match serde_json::from_str(&stdout) {
                Ok(v) => v,
                Err(e) => return (Res::Fail(format!("no JSON on stdout: {e}")), artifact, None),
            };
            let hash = v["trace_hash"].as_str().map(String::from);
            let mut fails = eval_run(&v, step.checks);
            fails.extend(check_outputs(ctx, step));
            (verdict(fails), artifact, hash)
        }
        Kind::Cmd => {
            let (ok, stdout, stderr) = match run_zynqsim(ctx, &args_out) {
                Ok(t) => t,
                Err(e) => return (Res::Fail(e), artifact, None),
            };
            if !ok {
                return (
                    Res::Fail(format!("exit code != 0: {}", last_line(&stderr))),
                    artifact,
                    None,
                );
            }
            let mut fails = Vec::new();
            for c in step.checks {
                if let Check::Stdout(t) = c {
                    if !stdout.contains(t) {
                        fails.push(format!("stdout does not contain `{t}`"));
                    }
                }
            }
            (verdict(fails), artifact, None)
        }
        Kind::Determinism => {
            let mut hashes = Vec::new();
            for _ in 0..3 {
                let args = vec![
                    "--json".to_string(),
                    "run".into(),
                    "--project".into(),
                    step.project.into(),
                    "--for".into(),
                    step.duration.into(),
                ];
                let (ok, stdout, _) = match run_zynqsim(ctx, &args) {
                    Ok(t) => t,
                    Err(e) => return (Res::Fail(e), artifact, None),
                };
                let v: Value = match (ok, serde_json::from_str(&stdout)) {
                    (true, Ok(v)) => v,
                    _ => return (Res::Fail("a run failed".into()), artifact, None),
                };
                hashes.push(v["trace_hash"].as_str().unwrap_or("").to_string());
            }
            if hashes.iter().all(|h| *h == hashes[0]) && !hashes[0].is_empty() {
                (Res::Pass, artifact, Some(hashes[0].clone()))
            } else {
                (
                    Res::Fail(format!("trace hashes differ: {}", hashes.join(" "))),
                    artifact,
                    None,
                )
            }
        }
        Kind::Mcp(script) => {
            let (res, detail) = run_mcp(ctx, script);
            if !detail.is_empty() {
                println!("{detail}");
            }
            (res, artifact, None)
        }
        Kind::Gui => {
            let mut args: Vec<String> = vec!["--project".into(), subst(step.project, &ctx.out)];
            args.extend(args_out);
            let status = Command::new(&ctx.gui)
                .args(&args)
                .current_dir(&ctx.root)
                .status();
            match status {
                Ok(s) if s.success() => (verdict(check_outputs(ctx, step)), artifact, None),
                Ok(s) => (Res::Fail(format!("zynqsim-gui exit {s}")), artifact, None),
                Err(e) => (
                    Res::Fail(format!("cannot run zynqsim-gui: {e}")),
                    artifact,
                    None,
                ),
            }
        }
    }
}

fn verdict(fails: Vec<String>) -> Res {
    if fails.is_empty() {
        Res::Pass
    } else {
        Res::Fail(fails.join("; "))
    }
}

/// Checks the artifact files a step must write.
fn check_outputs(ctx: &Ctx, step: &Step) -> Vec<String> {
    let mut fails = Vec::new();
    for (file, magic) in step.outputs {
        let path = ctx.out.join(file);
        match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => fails.push(format!("{file} is empty")),
            Ok(bytes) if !magic.ok(&bytes) => {
                fails.push(format!("{file} has the wrong {magic:?} header"))
            }
            Ok(_) => {}
            Err(e) => fails.push(format!("{file} not written: {e}")),
        }
    }
    fails
}

/// Drives `zynqsim-mcp` over stdio and checks each reply.
fn run_mcp(ctx: &Ctx, script: McpScript) -> (Res, String) {
    let requests = mcp_script(script, &ctx.root);
    let mut input = String::new();
    for (id, method, params) in &requests {
        let mut m = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            m["params"] = params.clone();
        }
        input.push_str(&m.to_string());
        input.push('\n');
        if *id == 1 {
            // The notification the protocol sends after `initialize`.
            input.push_str("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
        }
    }
    let child = Command::new(&ctx.mcp)
        .arg("--root")
        .arg(&ctx.root)
        .current_dir(&ctx.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return (
                Res::Fail(format!("cannot start zynqsim-mcp: {e}")),
                String::new(),
            );
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return (Res::Fail(format!("zynqsim-mcp failed: {e}")), String::new()),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let replies: Vec<Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let mut detail = String::new();
    let mut fails = Vec::new();
    for (id, method, _) in &requests {
        let reply = replies.iter().find(|r| r["id"] == json!(id));
        let Some(reply) = reply else {
            fails.push(format!("no reply to {method} (id {id})"));
            continue;
        };
        let _ = writeln!(
            detail,
            "    mcp {method} -> {}",
            compact(&reply.to_string())
        );
        match mcp_check(script, *id, reply) {
            Ok(_) => {}
            Err(e) => fails.push(format!("{method} (id {id}): {e}")),
        }
    }
    if matches!(script, McpScript::Tour) {
        let tools = replies
            .iter()
            .find(|r| r["id"] == json!(2))
            .and_then(|r| r["result"]["tools"].as_array().map(Vec::len))
            .unwrap_or(0);
        let _ = writeln!(detail, "    mcp tools/list: {tools} tools");
    }
    (verdict(fails), detail.trim_end().to_string())
}

/// Runs the selected steps and prints the table. Returns true when nothing failed.
pub fn run(root: &Path, opts: &Opts, build_firmware: &dyn Fn() -> bool) -> bool {
    let started = Instant::now();
    let out = root.join("demo-out");
    let _ = std::fs::create_dir_all(&out);
    let exe = |name: &str| {
        root.join("target/release")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    };
    let mut all = tour();
    if opts.gui {
        all.extend(gui_steps());
    }
    let steps = match select(all, opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask demo: {e}");
            return false;
        }
    };
    if !build_step(
        root,
        &[
            "build",
            "--release",
            "-p",
            "zynqsim",
            "-p",
            "zynqsim-gui",
            "-p",
            "zynqsim-mcp",
        ],
    ) {
        return false;
    }
    if !root.join("target/dpi/release/libxsim_dpi.a").exists()
        && !build_step(
            root,
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
        )
    {
        return false;
    }
    if !build_firmware() {
        return false;
    }
    let inspect = Command::new(exe("zynqsim"))
        .args(["--json", "inspect"])
        .current_dir(root)
        .output();
    let found = match inspect {
        Ok(o) => serde_json::from_slice::<Value>(&o.stdout)
            .map(|v| backend_found(&v))
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let ctx = Ctx {
        root: root.to_path_buf(),
        out: out.clone(),
        zynqsim: exe("zynqsim"),
        gui: exe("zynqsim-gui"),
        mcp: exe("zynqsim-mcp"),
        found,
    };
    println!(
        "zynqsim demo: {} step(s), output in {}",
        steps.len(),
        out.display()
    );
    let mut rows = Vec::new();
    let mut groups: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
    for step in &steps {
        let label = format!("[{}] {}", step.section, step.name);
        print!("{label:<32} ");
        let _ = std::io::stdout().flush();
        let t = Instant::now();
        let (res, artifact, hash) = run_step(&ctx, step);
        let wall = t.elapsed();
        println!("{} ({})", res.label(), fmt_wall(wall));
        if let Res::Fail(why) = &res {
            println!("    why: {why}");
        }
        if let (Some(g), Some(h)) = (step.hash_group, hash) {
            match groups.iter_mut().find(|(n, _)| *n == g) {
                Some((_, v)) => v.push((step.name.to_string(), h)),
                None => groups.push((g, vec![(step.name.to_string(), h)])),
            }
        }
        let example = if step.project.is_empty() {
            String::new()
        } else {
            step.project
                .trim_end_matches("/project.toml")
                .replace("examples/", "")
        };
        rows.push(Row {
            section: step.section,
            name: step.name.to_string(),
            example,
            res,
            wall,
            artifact,
        });
    }
    // Trace hashes of the same design through different backends.
    let mut group_ok = true;
    let mut group_lines = String::new();
    for (g, hashes) in &groups {
        match group_verdict(hashes) {
            Ok(line) => {
                let _ = writeln!(group_lines, "hash group {g}: identical: {line}");
            }
            Err(line) => {
                group_ok = false;
                let _ = writeln!(group_lines, "hash group {g}: DIFFERENT: {line}");
            }
        }
    }
    if !group_lines.is_empty() {
        println!("\n{group_lines}");
    }
    if !group_ok {
        rows.push(Row {
            section: 'z',
            name: "hash-groups".into(),
            example: String::new(),
            res: Res::Fail("trace hashes of one design differ across backends".into()),
            wall: Duration::ZERO,
            artifact: String::new(),
        });
    }
    let total = started.elapsed();
    let table_text = table(&rows, total);
    print!("\n{table_text}");
    println!("\nknown not working (not run):");
    for (name, why) in KNOWN_NOT_WORKING {
        println!("  {name}: {why}");
    }
    let _ = std::fs::write(
        out.join("summary.txt"),
        format!("{table_text}\n{group_lines}"),
    );
    rows.iter().all(|r| r.res.label() != "FAIL")
}

fn build_step(root: &Path, args: &[&str]) -> bool {
    eprintln!("\n==> cargo {}", args.join(" "));
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_the_flags() {
        assert_eq!(parse_args(&[]).unwrap(), Opts::default());
        let o = parse_args(&args(&["--quick"])).unwrap();
        assert!(o.quick && !o.gui && o.only.is_none());
        let o = parse_args(&args(&["--gui"])).unwrap();
        assert!(o.gui && !o.quick);
        let o = parse_args(&args(&["--only", "rust-hello, math-qemu,"])).unwrap();
        assert_eq!(
            o.only,
            Some(vec!["rust-hello".to_string(), "math-qemu".to_string()])
        );
        let o = parse_args(&args(&["--only=irq-demo"])).unwrap();
        assert_eq!(o.only, Some(vec!["irq-demo".to_string()]));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&["--fast"])).is_err());
        assert!(parse_args(&args(&["--only"])).is_err());
        assert!(parse_args(&args(&["--only", " , "])).is_err());
        assert!(parse_args(&args(&["--quick", "--only", "rust-hello"])).is_err());
    }

    #[test]
    fn quick_selects_the_subset_and_only_selects_by_name() {
        let quick = select(
            tour(),
            &Opts {
                quick: true,
                ..Opts::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = quick.iter().map(|s| s.name).collect();
        assert!(
            names.contains(&"rust-hello")
                && names.contains(&"rust-debug")
                && names.contains(&"mcp-tour")
        );
        assert!(
            !names.contains(&"board-demo"),
            "PL demos are not in --quick"
        );
        assert!(
            quick
                .iter()
                .any(|s| s.section == 'c' && s.name == "math-verilator")
        );

        let only = select(
            tour(),
            &Opts {
                only: Some(vec!["irq-demo".into()]),
                ..Opts::default()
            },
        )
        .unwrap();
        assert_eq!(only.len(), 1);
        let bad = select(
            tour(),
            &Opts {
                only: Some(vec!["nope".into()]),
                ..Opts::default()
            },
        );
        assert!(bad.unwrap_err().contains("unknown step `nope`"));
    }

    #[test]
    fn every_step_name_is_unique_and_has_a_section() {
        let steps = tour();
        let mut names: Vec<&str> = steps.iter().map(|s| s.name).collect();
        names.extend(gui_steps().iter().map(|s| s.name));
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate step names");
        for s in &steps {
            assert!(SECTIONS.iter().any(|(c, _)| *c == s.section), "{}", s.name);
        }
    }

    #[test]
    fn checks_are_evaluated_on_the_json_result() {
        let v = json!({
            "uart": {"1": "RESULT: ALL PASS\r\n", "0": ""},
            "leds": [{"duty": 255}, {"duty": 0}],
            "rgb": [[64, 128, 254], [254, 0, 32]],
            "debug": {"stopped": true, "source": {"line": 19}, "pc_symbol": "x::collatz_steps+0x1c"}
        });
        let ok = [
            Check::Uart1("ALL PASS"),
            Check::Json("/leds/0/duty", "255"),
            Check::Json("/rgb", "[[64,128,254],[254,0,32]]"),
            Check::Json("/debug/stopped", "true"),
            Check::Json("/debug/source/line", "19"),
            Check::JsonHas("/debug/pc_symbol", "collatz_steps"),
        ];
        assert!(eval_run(&v, &ok).is_empty());
        let bad = [
            Check::Uart1("FAIL"),
            Check::Json("/leds/0/duty", "0"),
            Check::Json("/missing", "1"),
            Check::JsonHas("/debug/pc_symbol", "nope"),
        ];
        assert_eq!(eval_run(&v, &bad).len(), 4);
    }

    #[test]
    fn magic_numbers_are_checked() {
        assert!(Magic::Png.ok(&[0x89, b'P', b'N', b'G', 0]));
        assert!(!Magic::Png.ok(b"GIF8"));
        assert!(Magic::Wav.ok(b"RIFF\0\0\0\0WAVEfmt "));
        assert!(!Magic::Wav.ok(b"RIFF"));
        assert!(Magic::Pcap.ok(&[0xa1, 0xb2, 0xc3, 0xd4, 0]));
        assert!(Magic::Pcap.ok(&[0xd4, 0xc3, 0xb2, 0xa1, 0]));
        assert!(!Magic::Pcap.ok(b"nope"));
    }

    #[test]
    fn hash_groups_must_agree() {
        let same = vec![
            ("a".into(), "fnv1a64:1".into()),
            ("b".into(), "fnv1a64:1".into()),
        ];
        assert!(group_verdict(&same).is_ok());
        let differ = vec![
            ("a".into(), "fnv1a64:1".into()),
            ("b".into(), "fnv1a64:2".into()),
        ];
        assert!(group_verdict(&differ).is_err());
        assert!(group_verdict(&[]).is_err());
    }

    #[test]
    fn backends_are_read_from_inspect() {
        let v = json!({"backends": [
            {"name": "qemu", "found": true},
            {"name": "icarus", "found": false},
            {"name": "demo"}
        ]});
        let found = backend_found(&v);
        assert_eq!(found.len(), 3);
        assert!(found.contains(&("qemu".to_string(), true)));
        assert!(found.contains(&("icarus".to_string(), false)));
    }

    #[test]
    fn the_table_counts_each_result() {
        let rows = vec![
            Row {
                section: 'a',
                name: "x".into(),
                example: "x".into(),
                res: Res::Pass,
                wall: Duration::from_millis(1500),
                artifact: String::new(),
            },
            Row {
                section: 'b',
                name: "y".into(),
                example: "y".into(),
                res: Res::Skip("Vivado XSim not found".into()),
                wall: Duration::ZERO,
                artifact: String::new(),
            },
            Row {
                section: 'c',
                name: "z".into(),
                example: "z".into(),
                res: Res::Fail("boom".into()),
                wall: Duration::ZERO,
                artifact: String::new(),
            },
        ];
        let t = table(&rows, Duration::from_secs(75));
        assert!(
            t.contains("total: 1 PASS, 1 FAIL, 1 SKIPPED in 1m15s"),
            "{t}"
        );
        assert!(t.contains("SKIPPED(Vivado XSim not found)"));
    }

    #[test]
    fn placeholders_point_into_the_output_folder() {
        let s = subst("JA={out}/oled.png", Path::new("C:\\repo\\demo-out"));
        assert_eq!(s, "JA=C:/repo/demo-out/oled.png");
    }
}
