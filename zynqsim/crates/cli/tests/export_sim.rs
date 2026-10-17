//! `zynqsim init --export-sim`: Vivado simulation exports become projects (ADR-017).
//!
//! The exports are real Vivado 2022.1 output (`fixtures/export`: `export_simulation`, a
//! `write_verilog -mode funcsim` netlist and the `launch_simulation -scripts_only` files of the
//! post-synthesis flow). The end-to-end tests need the Xilinx QEMU and Vivado XSim and print
//! `SKIPPED:` without them; the rest needs nothing.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-exp-{tag}-{}", std::process::id()));
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

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
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
    let lib =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/dpi/release/libxsim_dpi.a");
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
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

/// `init --export-sim SRC --out D` with the math firmware, then a 3 ms run. Returns the run JSON.
fn init_and_run(tag: &str, src: &str, expect_generated: bool) -> Value {
    let d = temp(tag);
    let export = fixtures().join("export").join(src);
    let elf = fixtures().join("elf/math_test.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        "--export-sim",
        export.to_str().unwrap(),
        "--out",
        d.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true, "{v}");
    assert_eq!(
        v["driven_module"].is_string(),
        expect_generated,
        "generated testbench or the export's own: {v}"
    );
    assert!(d.join("export-report.md").is_file());
    let toml = std::fs::read_to_string(d.join("project.toml")).unwrap();
    assert!(
        toml.contains("export_dir =") || toml.contains("filelist ="),
        "{toml}"
    );
    assert_eq!(d.join("tb/cosim_top.sv").is_file(), expect_generated);

    let r = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    let r: Value = serde_json::from_str(&stdout(&r)).unwrap();
    let uart = r["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(r["bus_accesses"], 21, "{r}");
    r
}

/// The Vivado `export_simulation` of the RTL design (its testbench is the export's own), the
/// post-synthesis functional netlist flow (`-L unisims_ver`, `glbl`), and a bare funcsim
/// netlist (testbench generated): the real firmware passes on each with the same 21 bus
/// accesses and the same trace.
#[test]
fn rtl_export_and_funcsim_netlist_pass_with_the_real_firmware_and_agree() {
    if !backend_found("qemu") || !backend_found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let rtl = init_and_run("rtl", "behav/xsim", false);
    let net = init_and_run("net", "postsynth", false);
    let bare = init_and_run("bare", "my_math_ip_funcsim.v", true);
    let digest = |v: &Value| v["trace_hash"].clone();
    assert!(digest(&rtl).is_string(), "{rtl}");
    assert_eq!(digest(&rtl), digest(&net), "netlist behaves like the RTL");
    assert_eq!(rtl["trace_len"], 329);
    assert_eq!(
        digest(&rtl),
        digest(&bare),
        "generated testbench route agrees too"
    );
    for (name, v) in [("rtl", &rtl), ("net", &net), ("bare", &bare)] {
        eprintln!("{name}: {} {}", v["time"], digest(v));
    }
}

#[test]
fn an_unreadable_export_names_file_line_and_construct() {
    let d = temp("bad");
    std::fs::write(d.join("a.v"), "module a; endmodule\n").unwrap();
    let list = d.join("x.prj");
    std::fs::write(
        &list,
        "verilog work \"a.v\"\nverilog work --frobnicate \"gone.v\"\n",
    )
    .unwrap();
    let out = d.join("proj");
    let o = zynqsim(&[
        "init",
        "--export-sim",
        list.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stdout(&o));
    let e = stderr(&o);
    assert!(e.contains("x.prj:2"), "{e}");
    assert!(e.contains("gone.v"), "{e}");
    assert!(!out.join("project.toml").exists());
}

#[test]
fn a_project_with_globs_and_an_export_validates_and_says_what_is_missing() {
    let d = temp("glob");
    let proj = d.join("p");
    std::fs::create_dir_all(proj.join("rtl")).unwrap();
    std::fs::create_dir_all(proj.join("fw")).unwrap();
    std::fs::copy(
        fixtures().join("elf/math_test.elf"),
        proj.join("fw/app.elf"),
    )
    .unwrap();
    std::fs::write(proj.join("rtl/a.v"), "module a; endmodule\n").unwrap();
    std::fs::write(proj.join("rtl/b.sv"), "module b; endmodule\n").unwrap();
    let export = fixtures()
        .join("export/behav/xsim")
        .display()
        .to_string()
        .replace('\\', "/");
    std::fs::write(
        proj.join("project.toml"),
        format!(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n\
             [backends]\nps = \"qemu\"\npl = \"xsim\"\n[cpu0]\nelf = \"fw/app.elf\"\n\
             [rtl]\nexport_dir = \"{export}\"\nfiles = [\"rtl/*.v\", \"rtl/*.sv\", \"nothing/*.vhd\"]\n\
             top = \"cosim_top\"\n"
        ),
    )
    .unwrap();
    let o = zynqsim(&[
        "--json",
        "inspect",
        "--project",
        proj.join("project.toml").to_str().unwrap(),
    ]);
    let s = stdout(&o);
    // The unmatched pattern is the only complaint: the two matching patterns and the export
    // (a clean read) are accepted.
    assert!(s.contains("nothing/*.vhd"), "{s}");
    assert!(s.contains("matches no files"), "{s}");
    assert!(!s.contains("rtl/*.v`"), "{s}");
    assert!(!s.contains("does not exist"), "{s}");
}
