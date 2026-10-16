//! The vendor-ip models through MCP: the AXI GPIO drives the board LEDs and reads switches, the
//! AXI Timer interrupts, and the AXI UART Lite is `uart_read`/`uart_write` port 2.
//!
//! Needs the Xilinx QEMU; without it the test prints `SKIPPED:`.

use serde_json::{Value, json};

fn server() -> zynqsim_mcp::McpServer {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    zynqsim_mcp::McpServer::new(vec![ws]).unwrap()
}

fn call(server: &mut zynqsim_mcp::McpServer, name: &str, args: Value) -> Value {
    server
        .handle(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": name, "arguments": args } }),
            &mut zynqsim_mcp::NoControl,
        )
        .unwrap()["result"]
        .clone()
}

fn leds(s: &mut zynqsim_mcp::McpServer) -> Vec<bool> {
    call(s, "board_get_leds", json!({}))["structuredContent"]["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["on"].as_bool().unwrap())
        .collect()
}

#[test]
fn vendor_ip_example_through_mcp() {
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

    let open = call(
        &mut s,
        "project_open",
        json!({ "path": "examples/gpio-timer-uart" }),
    );
    assert_eq!(open["isError"], false, "{open}");
    assert_eq!(open["structuredContent"]["simulation_started"], true);
    let st = call(&mut s, "status_get", json!({}));
    assert_eq!(st["structuredContent"]["status"]["pl_backend"], "native");
    let caps = st["structuredContent"]["status"]["capabilities"]
        .as_array()
        .unwrap();
    assert!(caps.iter().any(|c| c == "board_io"), "{st}");

    let r = call(
        &mut s,
        "board_set_switch",
        json!({ "id": "sw2", "on": true }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "sim_run", json!({ "duration": "6ms" }));
    assert_eq!(r["isError"], false, "{r}");

    let u1 = call(&mut s, "uart_read", json!({ "port": 1 }));
    let text = u1["structuredContent"]["text"].as_str().unwrap();
    assert!(text.contains("timer ticks=3"), "{text}");
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
    assert!(text.contains("sw=0x4 btn=0x0"), "{text}");
    // The PL's UART Lite is port 2.
    let u2 = call(&mut s, "uart_read", json!({ "port": 2 }));
    assert_eq!(u2["isError"], false, "{u2}");
    assert_eq!(
        u2["structuredContent"]["text"],
        "Hello from AXI UART Lite\r\n"
    );
    // The GPIO output is on the LEDs: switch 2 is mirrored onto LED 2.
    assert_eq!(leds(&mut s), vec![false, false, true, false]);

    // Host to PL UART and back.
    let w = call(&mut s, "uart_write", json!({ "port": 2, "text": "mcp" }));
    assert_eq!(w["isError"], false, "{w}");
    let r = call(&mut s, "sim_run", json!({ "duration": "3ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let u2 = call(&mut s, "uart_read", json!({ "port": 2 }));
    assert_eq!(u2["structuredContent"]["text"], "mcp");
    // A port the PL does not have is an error that says so.
    let r = call(&mut s, "uart_read", json!({ "port": 3 }));
    assert_eq!(r["isError"], true, "{r}");
    let msg = r["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("no uart 3"), "{msg}");

    // Snapshots and replay cover the models too.
    let h0 =
        call(&mut s, "status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    let r = call(&mut s, "snapshot_save", json!({ "name": "echoed" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "sim_run", json!({ "duration": "1ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "snapshot_restore", json!({ "name": "echoed" }));
    assert_eq!(r["isError"], false, "{r}");
    let h1 =
        call(&mut s, "status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    assert_eq!(h0, h1);
    let rep = call(&mut s, "session_replay", json!({}));
    assert_eq!(rep["structuredContent"]["deterministic"], true, "{rep}");
}
