//! The P3 exit demonstration (ADR-019): a Vivado-exported design and its BOOT.BIN, end to end.
//!
//! `fixtures/handoff` holds ONE Vivado 2022.1 block design (Zynq PS7 + the user RTL
//! `my_math_ip` as a module reference + AXI GPIO + AXI Timer + AXI UART Lite) as an XSA
//! (`write_hw_platform -fixed`) and as an `export_simulation` folder, and the `BOOT.BIN`
//! `bootgen` packed from the stub FSBL, a placeholder bitstream and `firmware/handoff_demo`.
//! A single `zynqsim init` makes the project; `zynqsim run` boots the firmware FROM the BOOT.BIN.
//!
//! The end-to-end tests need the Xilinx QEMU, Vivado XSim and the DPI library and print
//! `SKIPPED:` without them; the `init` tests need nothing.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-handoff-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn handoff(rel: &str) -> PathBuf {
    root().join("fixtures/handoff").join(rel)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn backend_found(name: &str) -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == name && b["found"] == true)
}

fn ensure_dpi_lib() {
    let lib = root().join("target/dpi/release/libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
            ])
            .current_dir(root())
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

/// The two routes to the same project: the PL from the XSA plus `--rtl`, or the user RTL and
/// testbench from the `export_simulation` folder (the Xilinx IP always from the XSA).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Variant {
    Xsa,
    Export,
    /// The XSA variant booting the firmware ELF directly instead of the BOOT.BIN.
    XsaElf,
}

fn init(variant: Variant, dir: &Path) -> Value {
    let xsa = handoff("design.xsa");
    let rtl = handoff("rtl/my_math_ip.v");
    let export = handoff("export/xsim");
    let bin = handoff("BOOT.BIN");
    let elf = root().join("fixtures/elf/rust_handoff_demo.elf");
    let mut args = vec!["--json", "init", "--xsa", s(&xsa), "--out", s(dir)];
    match variant {
        Variant::Xsa => args.extend(["--rtl", s(&rtl), "--boot-bin", s(&bin)]),
        Variant::Export => args.extend(["--export-sim", s(&export), "--boot-bin", s(&bin)]),
        Variant::XsaElf => args.extend(["--rtl", s(&rtl), "--elf", s(&elf)]),
    }
    let o = zynqsim(&args);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true, "{v}");
    v
}

fn run(project: &Path) -> Value {
    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        s(&project.join("project.toml")),
        "--for",
        "8ms",
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    serde_json::from_str(&stdout(&r)).unwrap()
}

/// What the firmware must have printed and what the PL must have shown.
fn assert_all_pass(r: &Value) {
    let u1 = r["uart"]["1"].as_str().unwrap();
    assert_eq!(
        u1.matches("math PASS").count(),
        7,
        "seven math vectors:\n{u1}"
    );
    assert!(!u1.contains("FAIL"), "{u1}");
    assert!(
        u1.contains("math PASS in=0x12345678 out=0x2468acf0"),
        "{u1}"
    );
    assert!(u1.contains("gpio: tri=0x0 data=0x9"), "{u1}");
    assert!(u1.contains("timer ticks=3"), "{u1}");
    assert!(u1.contains("uartlite line sent, drained=true"), "{u1}");
    assert!(u1.contains("RESULT: ALL PASS"), "{u1}");
    assert!(u1.contains("COSIM_DONE"), "{u1}");
    // The UART Lite line, as the PL sent it (port 2 = the first UART Lite).
    assert_eq!(r["uart"]["pl"]["2"], "HANDOFF UART OK\r\n");
    // GPIO bits 0 and 3 on LED0 and LED3.
    let duty: Vec<u64> = r["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["duty"].as_u64().unwrap())
        .collect();
    assert_eq!(duty, vec![255, 0, 0, 255], "{r}");
    assert_eq!(r["backend"]["pl"], "xsim+native");
}

fn tools_present() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return false;
    }
    ensure_dpi_lib();
    true
}

/// One `init` gives the XSA-derived PL mapping AND the BOOT.BIN boot. Needs no tools.
#[test]
fn one_init_gives_the_xsa_mapping_and_the_bootbin_boot() {
    let d = temp("init-xsa");
    let v = init(Variant::Xsa, &d);
    assert_eq!(v["boot"]["mode"], "boot_bin");
    assert_eq!(v["boot"]["file"], "BOOT.BIN");
    assert_eq!(v["testbench_module"], "my_math_ip");
    assert!(d.join("BOOT.BIN").is_file());
    assert_eq!(
        std::fs::read(d.join("BOOT.BIN")).unwrap(),
        std::fs::read(handoff("BOOT.BIN")).unwrap()
    );
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("mode = \"boot_bin\""), "{toml}");
    assert!(!toml.contains("[cpu0]"), "{toml}");
    for needle in [
        "\"u_math.S_AXI\" = \"M_AXI_GP0@0x43C00000\"",
        "\"axi_gpio_0.S_AXI\" = \"M_AXI_GP0@0x41200000\"",
        "\"axi_timer_0.S_AXI\" = \"M_AXI_GP0@0x42800000\"",
        "\"axi_uartlite_0.S_AXI\" = \"M_AXI_GP0@0x40600000\"",
        "\"axi_timer_0.interrupt\" = \"IRQ_F2P[0]\"",
        "\"axi_uartlite_0.interrupt\" = \"IRQ_F2P[1]\"",
        "\"axi_gpio_0.GPIO\" = \"board.leds\"",
        "axi_timer_0 = \"axi_timer\"",
        "axi_uartlite_0 = \"axi_uartlite\"",
    ] {
        assert!(toml.contains(needle), "missing `{needle}`:\n{toml}");
    }
    let report = std::fs::read_to_string(d.join("xsa-report.md")).unwrap();
    assert!(report.contains("not executed"), "{report}");
    assert!(!report.contains("NOT RUNNABLE"), "{report}");
    let o = zynqsim(&["inspect", "--project", s(&d.join("project.toml"))]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
}

/// The export variant: the user RTL and testbench are referenced from the `export_simulation`
/// folder, the Xilinx IP still come from the XSA, the image is the same BOOT.BIN.
#[test]
fn the_export_variant_references_the_export_and_keeps_the_xilinx_ip() {
    // The export's lists name `$XILINX_VIVADO/.../glbl.v`: without a Vivado
    // install (CI) the variable is undefined and the export is refused.
    if !backend_found("xsim") && std::env::var_os("XILINX_VIVADO").is_none() {
        eprintln!("SKIPPED: the export names $XILINX_VIVADO (no Vivado here)");
        return;
    }
    let d = temp("init-export");
    let v = init(Variant::Export, &d);
    assert_eq!(v["boot"]["mode"], "boot_bin");
    assert!(
        v["rtl"]["export_dir"]
            .as_str()
            .unwrap()
            .ends_with("fixtures/handoff/export/xsim"),
        "{v}"
    );
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(toml.contains("export_dir ="), "{toml}");
    assert!(toml.contains("mode = \"boot_bin\""), "{toml}");
    assert!(toml.contains("axi_gpio_0 = \"axi_gpio\""), "{toml}");
    assert!(
        !d.join("tb/cosim_top.sv").exists(),
        "the export's own testbench is used"
    );
    let report = std::fs::read_to_string(d.join("xsa-report.md")).unwrap();
    assert!(report.contains("export_dir"), "{report}");
    assert!(report.contains("own testbench"), "{report}");
    let o = zynqsim(&["inspect", "--project", s(&d.join("project.toml"))]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));

    // The export alone (no XSA) with the same image: still one init, but the PL is the user
    // RTL only. It validates; whether firmware that wants the Xilinx IP passes is not claimed.
    let alone = temp("init-export-alone");
    let o = zynqsim(&[
        "--json",
        "init",
        "--export-sim",
        s(&handoff("export/xsim")),
        "--boot-bin",
        s(&handoff("BOOT.BIN")),
        "--out",
        s(&alone),
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let t = std::fs::read_to_string(alone.join("project.toml")).unwrap();
    assert!(
        t.contains("mode = \"boot_bin\"") && t.contains("export_dir ="),
        "{t}"
    );
    assert!(!t.contains("[cpu0]"), "{t}");
    assert!(alone.join("BOOT.BIN").is_file());
}

/// Both existing paths still work unchanged: `init --boot` alone, and `init --xsa` with an ELF.
#[test]
fn init_boot_and_init_xsa_keep_working_separately() {
    let d = temp("separate");
    let o = zynqsim(&[
        "init",
        "--boot",
        s(&handoff("BOOT.BIN")),
        "--out",
        s(&d.join("boot")),
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let t = std::fs::read_to_string(d.join("boot/project.toml")).unwrap();
    assert!(t.contains("mode = \"boot_bin\""), "{t}");
    assert!(!t.contains("axi_gpio"), "{t}");
    let v = init(Variant::XsaElf, &d.join("elf"));
    assert_eq!(v["boot"]["mode"], "elf");
    let t = std::fs::read_to_string(d.join("elf/project.toml")).unwrap();
    assert!(t.contains("[cpu0]") && !t.contains("BOOT.BIN"), "{t}");
}

/// The path under test, end to end: the firmware boots FROM the BOOT.BIN, talks to the user
/// RTL in XSim and to the vendor models, and prints ALL PASS. Both variants, the same bytes
/// and the same trace digest; booting the same firmware as an ELF changes neither; three runs
/// of the BOOT.BIN project give one digest.
#[test]
fn the_exit_design_runs_end_to_end_from_the_bootbin_in_both_variants() {
    if !tools_present() {
        return;
    }
    let xsa_dir = temp("e2e-xsa");
    init(Variant::Xsa, &xsa_dir);
    let first = run(&xsa_dir);
    assert_all_pass(&first);
    let hash = first["trace_hash"].as_str().unwrap().to_string();
    let uart = first["uart"]["1"].as_str().unwrap().to_string();
    eprintln!(
        "xsa variant: {} records, {} bus accesses, {hash}\n{uart}",
        first["trace_len"], first["bus_accesses"]
    );

    // Determinism: three runs of the same project, one digest, one text.
    for i in 2..=3 {
        let again = run(&xsa_dir);
        assert_eq!(again["trace_hash"], hash.as_str(), "run {i}");
        assert_eq!(again["uart"]["1"], uart.as_str(), "run {i}");
        assert_eq!(again["trace_len"], first["trace_len"], "run {i}");
    }

    // The committed example is the same project (made by the `init` of its README).
    let example = run(&root().join("examples/vivado-handoff"));
    assert_all_pass(&example);
    assert_eq!(
        example["trace_hash"],
        hash.as_str(),
        "the committed example"
    );

    // The PL from the export_simulation folder: identical text, identical digest.
    let exp_dir = temp("e2e-export");
    init(Variant::Export, &exp_dir);
    let exp = run(&exp_dir);
    assert_all_pass(&exp);
    assert_eq!(exp["uart"]["1"], uart.as_str(), "same UART text");
    assert_eq!(exp["trace_hash"], hash.as_str(), "same trace digest");
    assert_eq!(exp["trace_len"], first["trace_len"]);
    assert_eq!(exp["bus_accesses"], first["bus_accesses"]);

    // The same firmware booted as an ELF (no BOOT.BIN): the BOOT.BIN route changes nothing but
    // the file name in the `qemu start elf=...` trace note (so the digests differ), which the
    // Perfetto export shows: every other record is identical.
    let elf_dir = temp("e2e-elf");
    init(Variant::XsaElf, &elf_dir);
    let direct = run(&elf_dir);
    assert_all_pass(&direct);
    assert_eq!(direct["uart"]["1"], uart.as_str(), "ELF boot, same text");
    assert_eq!(direct["trace_len"], first["trace_len"]);
    assert_eq!(direct["bus_accesses"], first["bus_accesses"]);
    let events = |project: &Path| -> Vec<Value> {
        let f = project.join("trace.json");
        let r = zynqsim(&[
            "run",
            "--project",
            s(&project.join("project.toml")),
            "--for",
            "8ms",
            "--perfetto",
            s(&f),
        ]);
        assert_eq!(r.status.code(), Some(0), "{}", stderr(&r));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        v["traceEvents"].as_array().unwrap().clone()
    };
    let (a, b) = (events(&xsa_dir), events(&elf_dir));
    assert_eq!(a.len(), b.len());
    let differing: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    assert_eq!(differing, vec![0], "only the qemu start note differs");
    assert!(
        a[0]["name"].as_str().unwrap().contains("bootbin-app.elf")
            && b[0]["name"]
                .as_str()
                .unwrap()
                .contains("rust_handoff_demo.elf"),
        "{} / {}",
        a[0],
        b[0]
    );
}
