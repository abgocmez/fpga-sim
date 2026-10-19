//! `project_create_from_xsa` through MCP: a real Vivado 2022.1 handoff becomes a project, the
//! unmodelled IP is reported and keeps `project_open` from starting a simulation.
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
    let rel = format!("target/xsa-mcp-{tag}-{}", std::process::id());
    let abs = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&rel);
    let _ = std::fs::remove_dir_all(&abs);
    rel
}

#[test]
fn an_xsa_with_an_unmodelled_ip_creates_a_project_that_will_not_open() {
    let mut s = server();
    let out = out_dir("hwicap");
    let r = call(
        &mut s,
        "project_create_from_xsa",
        json!({
            "xsa_path": "fixtures/xsa/zs_math_hwicap.hwh",
            "out_dir": out,
            "rtl": ["fixtures/rtl/math/my_math_ip.v"],
            "elf": "fixtures/elf/math_test.elf",
        }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["design"], "design_hwicap");
    assert_eq!(v["vivado_version"], "2022.1");
    assert_eq!(v["runnable"], false);
    assert_eq!(v["testbench_module"], "my_math_ip");
    assert_eq!(v["ps7"]["fclk"][0]["enabled"], true);
    let un = v["unmodelled"].as_array().unwrap();
    assert_eq!(un.len(), 1, "{un:?}");
    assert_eq!(un[0]["instance"], "axi_hwicap_0");
    assert_eq!(un[0]["base"], "0x43000000");
    let irqs = v["interrupts"].as_array().unwrap();
    assert_eq!(irqs.len(), 1);
    assert_eq!(irqs[0]["bit"], 0);
    let inst: Vec<&str> = v["pl_instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["instance"].as_str().unwrap())
        .collect();
    assert!(
        inst.contains(&"u_math") && inst.contains(&"axi_hwicap_0"),
        "{inst:?}"
    );
    assert!(
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["field"] == "xsa.unmodelled.axi_hwicap_0" && d["severity"] == "error"),
        "{v}"
    );

    // Files exist and the report is the one the tool names.
    let report = std::path::PathBuf::from(v["report"].as_str().unwrap());
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains("NOT RUNNABLE"), "{text}");

    // project_validate agrees; project_open refuses to start a simulation.
    let val = call(&mut s, "project_validate", json!({ "path": out }));
    assert_eq!(val["structuredContent"]["valid"], false, "{val}");
    let open = call(&mut s, "project_open", json!({ "path": out }));
    assert_eq!(open["isError"], false, "{open}");
    assert_eq!(open["structuredContent"]["simulation_started"], false);
    assert_eq!(open["structuredContent"]["errors"], 1);

    // Creating again does not overwrite.
    let again = call(
        &mut s,
        "project_create_from_xsa",
        json!({ "xsa_path": "fixtures/xsa/zs_math_uart16550.hwh", "out_dir": out }),
    );
    assert_eq!(again["isError"], true, "{again}");
    assert!(
        again["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{again}"
    );
}

#[test]
fn argument_errors_are_precise() {
    let mut s = server();
    let r = call(
        &mut s,
        "project_create_from_xsa",
        json!({ "xsa_path": "fixtures/xsa/nope.xsa", "out_dir": out_dir("nope") }),
    );
    assert_eq!(r["isError"], true);
    assert!(
        r["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not exist"),
        "{r}"
    );
    let r = call(
        &mut s,
        "project_create_from_xsa",
        json!({ "xsa_path": "fixtures/xsa/zs_math_gpio.xsa", "out_dir": "/tmp-outside/x" }),
    );
    assert_eq!(r["isError"], true);
    let r = call(
        &mut s,
        "project_create_from_xsa",
        json!({ "xsa_path": "fixtures/xsa/zs_math_gpio.xsa", "out_dir": out_dir("badrtl"), "rtl": [1] }),
    );
    assert_eq!(r["isError"], true);
    assert!(
        r["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("`rtl[0]` must be string"),
        "{r}"
    );
}
