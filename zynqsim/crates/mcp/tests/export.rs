//! `project_create_from_export` through MCP: a real Vivado 2022.1 simulation export (and a bare
//! funcsim netlist) becomes a valid project; unsupported constructs come back with file, line
//! and construct.
//!
//! Needs no external tool.

use serde_json::{Value, json};

fn server() -> zynqsim_mcp::McpServer {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    zynqsim_mcp::McpServer::new(vec![ws]).unwrap()
}

fn call(s: &mut zynqsim_mcp::McpServer, name: &str, args: Value) -> Value {
    s.handle(
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": args } }),
        &mut zynqsim_mcp::NoControl,
    )
    .unwrap()["result"]
        .clone()
}

fn out_dir(tag: &str) -> String {
    let rel = format!("target/export-mcp-{tag}-{}", std::process::id());
    let abs = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&rel);
    let _ = std::fs::remove_dir_all(&abs);
    rel
}

#[test]
fn the_vivado_export_and_the_netlist_become_valid_projects() {
    let mut s = server();
    let out = out_dir("rtl");
    let r = call(
        &mut s,
        "project_create_from_export",
        json!({
            "export_path": "fixtures/export/behav/xsim",
            "out_dir": out,
            "elf": "fixtures/elf/math_test.elf",
        }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["runnable"], true, "{v}");
    assert_eq!(v["top"], "cosim_top");
    assert!(
        v["driven_module"].is_null(),
        "the export's own testbench: {v}"
    );
    let src: Vec<(&str, &str, &str)> = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["path"].as_str().unwrap().rsplit('/').next().unwrap(),
                s["language"].as_str().unwrap(),
                s["library"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        src,
        [
            ("my_math_ip.v", "verilog", "xil_defaultlib"),
            ("cosim_top.sv", "sv", "xil_defaultlib"),
            ("glbl.v", "verilog", "xil_defaultlib"),
        ]
    );
    let libs: Vec<&str> = v["link_libraries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(
        libs.contains(&"unisims_ver") && libs.contains(&"secureip"),
        "{libs:?}"
    );
    let text = std::fs::read_to_string(v["report"].as_str().unwrap()).unwrap();
    assert!(
        text.contains("unisims_ver") && text.contains("cosim_top"),
        "{text}"
    );
    let val = call(&mut s, "project_validate", json!({ "path": out }));
    assert_eq!(val["structuredContent"]["valid"], true, "{val}");

    // The bare netlist gets a generated testbench around the AXI4-Lite slave in it.
    let out2 = out_dir("net");
    let r = call(
        &mut s,
        "project_create_from_export",
        json!({
            "export_path": "fixtures/export/my_math_ip_funcsim.v",
            "out_dir": out2,
            "elf": "fixtures/elf/math_test.elf",
        }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["driven_module"], "my_math_ip", "{v}");
    assert_eq!(v["top"], "cosim_top");
    assert_eq!(v["runnable"], true, "{v}");

    // Not overwritten twice.
    let again = call(
        &mut s,
        "project_create_from_export",
        json!({ "export_path": "fixtures/export/behav/xsim", "out_dir": out }),
    );
    assert_eq!(again["isError"], true, "{again}");
}

#[test]
fn an_export_that_cannot_be_read_is_an_error_with_file_and_line() {
    let mut s = server();
    let out = out_dir("bad");
    // A list in the output tree that names a file that is not there.
    let list_rel = format!("{out}-list.prj");
    let list = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&list_rel);
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    std::fs::write(&list, "verilog work \"missing_file.v\"\n").unwrap();
    let r = call(
        &mut s,
        "project_create_from_export",
        json!({ "export_path": list_rel, "out_dir": out }),
    );
    assert_eq!(r["isError"], true, "{r}");
    let m = r["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(m.contains(".prj:1") && m.contains("missing_file.v"), "{m}");
}
