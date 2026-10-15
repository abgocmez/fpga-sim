//! The SD card and QSPI flash example projects driven through MCP: the status
//! reports what is attached, and the firmware's output reaches `uart_read`.
//!
//! Needs the Xilinx QEMU (no Vivado); prints `SKIPPED:` otherwise.

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

fn uart(s: &mut zynqsim_mcp::McpServer) -> String {
    let r = call(s, "uart_read", json!({ "port": 1 }));
    r["structuredContent"]["text"]
        .as_str()
        .unwrap_or_default()
        .replace("\r\n", "\n")
}

#[test]
fn sd_and_qspi_demo_projects_through_mcp() {
    let mut s = server();
    let backends = call(&mut s, "backend_list", json!({}));
    let have_qemu = backends["structuredContent"]["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true);
    if !have_qemu {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }

    // --- SD card -------------------------------------------------------
    let open = call(
        &mut s,
        "project_open",
        json!({ "path": "examples/sd-demo" }),
    );
    assert_eq!(open["isError"], false, "{open}");
    let st = call(&mut s, "status_get", json!({}))["structuredContent"]["status"].clone();
    let caps: Vec<&str> = st["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(caps.contains(&"storage"), "{caps:?}");
    assert!(
        caps.contains(&"snapshot"),
        "read-only storage keeps snapshots: {caps:?}"
    );
    assert_eq!(st["storage"]["sd"]["source"], "dir:sdcard");
    assert_eq!(st["storage"]["sd"]["writable"], false);
    assert!(st["storage"]["qspi"].is_null(), "{st}");

    let run = call(&mut s, "sim_run", json!({ "duration": "2ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let text = uart(&mut s);
    assert!(
        text.contains("Hello from the simulated microSD card!"),
        "{text}"
    );
    assert!(text.contains("RESULT: ALL PASS"), "{text}");

    // Snapshots re-execute against the same card.
    let hash =
        call(&mut s, "status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    let r = call(&mut s, "snapshot_save", json!({ "name": "done" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "snapshot_restore", json!({ "name": "done" }));
    assert_eq!(r["isError"], false, "{r}");
    let again =
        call(&mut s, "status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    assert_eq!(again, hash);

    // --- QSPI flash ----------------------------------------------------
    let open = call(
        &mut s,
        "project_open",
        json!({ "path": "examples/qspi-demo" }),
    );
    assert_eq!(open["isError"], false, "{open}");
    let st = call(&mut s, "status_get", json!({}))["structuredContent"]["status"].clone();
    assert_eq!(st["storage"]["qspi"]["model"], "s25fl129p0");
    assert_eq!(st["storage"]["qspi"]["bytes"], 16 << 20);
    assert!(st["storage"]["sd"].is_null(), "{st}");
    let run = call(&mut s, "sim_run", json!({ "duration": "2ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let text = uart(&mut s);
    assert!(text.contains("RDID: 01 20 18 4d 00"), "{text}");
    assert!(text.contains("FIFO and linear reads agree"), "{text}");
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
}
