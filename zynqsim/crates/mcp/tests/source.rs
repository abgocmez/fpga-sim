//! Source-level debugging through MCP: `cpu_regs` shows the line, `source_line`
//! returns context, `breakpoint_set` takes `file.rs:LINE`.
//!
//! Needs the Xilinx QEMU (no Vivado); prints `SKIPPED:` otherwise.

use serde_json::{Value, json};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn mark(name: &str) -> u64 {
    std::fs::read_to_string(root().join("firmware/debug_demo/src/main.rs"))
        .unwrap()
        .lines()
        .position(|l| l.contains(&format!("MARK:{name}")))
        .map(|i| i as u64 + 1)
        .unwrap_or_else(|| panic!("MARK:{name}"))
}

#[test]
fn source_lines_through_mcp() {
    let mut server = zynqsim_mcp::McpServer::new(vec![root()]).unwrap();
    let mut call = |name: &str, args: Value| -> Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    if !backends
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
    {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let step = mark("collatz-step");
    let call_sum = mark("call-checksum");

    let open = call("project_open", json!({ "path": "examples/rust-debug" }));
    assert_eq!(open["isError"], false, "{open}");

    // The line breakpoint from project.toml stops the run.
    let run = call("sim_run", json!({ "duration": "1ms" }));
    assert_eq!(run["isError"], false, "{run}");
    assert_eq!(run["structuredContent"]["debug"]["stopped"], true, "{run}");
    let reason = run["structuredContent"]["debug"]["reason"].as_str().unwrap();
    assert!(reason.contains(&format!("main.rs:{step}")), "{reason}");

    // cpu_regs shows file:line and the lines around it.
    let r = call("cpu_regs", json!({}));
    assert_eq!(r["isError"], false, "{r}");
    let sc = &r["structuredContent"];
    assert_eq!(sc["pc_source"]["line"], step, "{sc}");
    assert!(
        sc["pc_source"]["file"]
            .as_str()
            .unwrap()
            .ends_with("debug_demo/src/main.rs")
    );
    let lines = sc["source_context"]["lines"].as_array().unwrap();
    let cur: Vec<&Value> = lines.iter().filter(|l| l["current"] == true).collect();
    assert_eq!(cur.len(), 1);
    assert_eq!(cur[0]["line"], step);
    assert!(cur[0]["text"].as_str().unwrap().contains("MARK:collatz-step"));

    // source_line by address (the pc), with a small context.
    let pc = sc["regs"]["pc"].as_str().unwrap();
    let pc = u64::from_str_radix(pc.trim_start_matches("0x"), 16).unwrap();
    let s = call("source_line", json!({ "addr": pc, "context": 1 }));
    assert_eq!(s["isError"], false, "{s}");
    let ss = &s["structuredContent"];
    assert_eq!(ss["line"], step);
    assert_eq!(ss["function"], "collatz_steps");
    assert!(ss["column"].as_u64().unwrap() > 0);
    assert_eq!(ss["lines"].as_array().unwrap().len(), 3);

    // By symbol and by file:line.
    let s = call("source_line", json!({ "symbol": "main" }));
    assert_eq!(s["isError"], false, "{s}");
    assert_eq!(s["structuredContent"]["function"], "main");
    let s = call(
        "source_line",
        json!({ "location": format!("main.rs:{call_sum}"), "context": 0 }),
    );
    assert_eq!(s["structuredContent"]["line"], call_sum, "{s}");
    assert_eq!(s["structuredContent"]["lines"].as_array().unwrap().len(), 1);
    // Nothing to resolve: a clear error.
    let s = call("source_line", json!({}));
    assert_eq!(s["isError"], true);
    let s = call("source_line", json!({ "location": "nosuch.rs:3" }));
    assert_eq!(s["isError"], true, "{s}");

    // breakpoint_set with a source location: clear the loop line, set another.
    let b = call(
        "breakpoint_set",
        json!({ "location": format!("main.rs:{step}"), "enable": false }),
    );
    assert_eq!(b["isError"], false, "{b}");
    let b = call(
        "breakpoint_set",
        json!({ "location": format!("main.rs:{call_sum}") }),
    );
    assert_eq!(b["isError"], false, "{b}");
    assert_eq!(b["structuredContent"]["breakpoints"][0]["source"]["line"], call_sum);
    let list = call("breakpoint_list", json!({}));
    let bps = list["structuredContent"]["breakpoints"].as_array().unwrap();
    assert!(
        bps.iter().any(|b| b["source"]["line"] == call_sum),
        "{list}"
    );
    assert!(bps.iter().all(|b| b["source"]["line"] != step), "{list}");

    let run = call("sim_run", json!({ "duration": "1ms" }));
    assert_eq!(run["structuredContent"]["debug"]["stopped"], true, "{run}");
    let r = call("cpu_regs", json!({}));
    assert_eq!(r["structuredContent"]["pc_source"]["line"], call_sum, "{r}");
}
