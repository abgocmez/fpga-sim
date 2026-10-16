//! `zynqsim rtl-check`: diagnostics, exit codes, the JSON hierarchy and the
//! include and define options of the native RTL front end.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code")
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-rtl-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

#[test]
fn a_clean_design_exits_zero_and_json_lists_the_hierarchy_and_ports() {
    let f = fixture("fixtures/rtl/math/my_math_ip.v");
    let o = zynqsim(&[
        "--json",
        "rtl-check",
        f.to_str().unwrap(),
        "--top",
        "my_math_ip",
    ]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_str(stdout(&o).trim()).expect("JSON");
    assert_eq!(v["ok"], Value::Bool(true));
    assert_eq!(v["top"], "my_math_ip");
    assert_eq!(v["hierarchy"]["module"], "my_math_ip");
    let ports = v["hierarchy"]["ports"].as_array().expect("ports");
    assert!(
        ports.len() >= 20,
        "the fixture has 21 ports, got {}",
        ports.len()
    );
    let clk = ports
        .iter()
        .find(|p| p["name"] == "S_AXI_ACLK")
        .expect("clock port");
    assert_eq!(clk["dir"], "input");
    assert_eq!(clk["width"], 1);
}

#[test]
fn without_top_json_lists_every_module_and_no_hierarchy() {
    let f = fixture("fixtures/rtl/math/my_math_ip.v");
    let o = zynqsim(&["--json", "rtl-check", f.to_str().unwrap()]);
    assert_eq!(code(&o), 0);
    let v: Value = serde_json::from_str(stdout(&o).trim()).expect("JSON");
    assert!(v["hierarchy"].is_null());
    assert_eq!(v["modules"][0]["name"], "my_math_ip");
}

#[test]
fn diagnostics_are_printed_as_file_line_column_construct_message() {
    let dir = temp("diag");
    let f = dir.join("bad.sv");
    std::fs::write(
        &f,
        "module m;\ninterface bus_if;\nendinterface\nendmodule\n",
    )
    .unwrap();
    let o = zynqsim(&["rtl-check", f.to_str().unwrap()]);
    assert_eq!(code(&o), 1, "unsupported constructs exit 1");
    let text = stdout(&o);
    let line = text
        .lines()
        .find(|l| l.contains(": interface:"))
        .unwrap_or_else(|| panic!("no interface diagnostic in:\n{text}"));
    assert!(line.contains("bad.sv:2:1: interface: "), "{line}");
    assert!(
        text.contains("diagnostic(s)"),
        "a summary line follows:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn include_directories_and_defines_are_honoured() {
    let dir = temp("inc");
    let inc = dir.join("inc");
    std::fs::create_dir_all(&inc).unwrap();
    std::fs::write(inc.join("widths.vh"), "`define DATA_W 8\n").unwrap();
    let f = dir.join("top.sv");
    std::fs::write(
        &f,
        "`include \"widths.vh\"\nmodule top(input [`DATA_W-1:0] d, input [`WIDTH-1:0] w);\nendmodule\n",
    )
    .unwrap();
    let o = zynqsim(&[
        "--json",
        "rtl-check",
        f.to_str().unwrap(),
        "--top",
        "top",
        "-I",
        inc.to_str().unwrap(),
        "-D",
        "WIDTH=12",
    ]);
    assert_eq!(
        code(&o),
        0,
        "{}{}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(stdout(&o).trim()).expect("JSON");
    let ports = v["hierarchy"]["ports"].as_array().unwrap();
    assert_eq!(ports[0]["width"], 8, "the include supplies DATA_W");
    assert_eq!(ports[1]["width"], 12, "-D supplies WIDTH");

    // Without the include directory the same file is reported.
    let o = zynqsim(&["rtl-check", f.to_str().unwrap(), "-D", "WIDTH=12"]);
    assert_eq!(code(&o), 1);
    assert!(stdout(&o).contains("cannot find included file `widths.vh`"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_is_a_usage_error() {
    let o = zynqsim(&["rtl-check", "no/such/file.sv"]);
    assert_eq!(code(&o), 2);
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot read"));
}
