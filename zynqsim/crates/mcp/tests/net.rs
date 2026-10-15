//! The board's Ethernet through the MCP server: capture, counters, the
//! capability gate, snapshot/restore and replay of a network session.
//!
//! Needs the Xilinx QEMU; without it the QEMU test prints `SKIPPED:`.

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

#[test]
fn net_tools_are_refused_with_the_reason_without_a_network() {
    let mut s = server();
    // The built-in demo has no Ethernet: the gate says so for both tools.
    for (tool, args) in [
        ("net_capture", json!({})),
        ("net_send", json!({ "hex": "ffffffffffff" })),
    ] {
        let r = call(&mut s, tool, args);
        assert_eq!(r["isError"], true, "{tool}: {r}");
        let msg = r["structuredContent"]["error"]["message"].as_str().unwrap();
        assert!(msg.contains("Network"), "{tool}: {msg}");
    }
    let st = call(&mut s, "status_get", json!({}));
    let caps = st["structuredContent"]["status"]["capabilities"]
        .as_array()
        .unwrap();
    assert!(!caps.iter().any(|c| c == "network"));
    assert!(st["structuredContent"].get("network").is_none());
}

#[test]
fn network_project_through_mcp() {
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
        json!({ "path": "examples/net-demo" }),
    );
    assert_eq!(open["isError"], false, "{open}");
    assert_eq!(open["structuredContent"]["simulation_started"], true);

    // The capability appears with the network, and the counters start at zero.
    let st = call(&mut s, "status_get", json!({}));
    let caps = st["structuredContent"]["status"]["capabilities"]
        .as_array()
        .unwrap();
    assert!(caps.iter().any(|c| c == "network"), "{st}");
    assert_eq!(st["structuredContent"]["network"]["frames_tx"], 0);

    let run = call(&mut s, "sim_run", json!({ "duration": "10ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let uart = call(&mut s, "uart_read", json!({ "port": 1 }));
    let text = uart["structuredContent"]["text"].as_str().unwrap();
    assert!(text.contains("ping reply seq=3 bytes=32"), "{text}");
    assert!(text.contains("RESULT: ALL PASS"), "{text}");

    // The capture: fourteen frames, summarised, with counters.
    let cap = call(&mut s, "net_capture", json!({}));
    assert_eq!(cap["isError"], false, "{cap}");
    let c = &cap["structuredContent"];
    assert_eq!(c["total"], 14);
    assert_eq!(c["stats"]["frames_tx"], 7);
    assert_eq!(c["stats"]["frames_rx"], 7);
    assert_eq!(c["stats"]["icmp"], 8);
    let frames = c["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 14);
    assert_eq!(frames[0]["direction"], "tx");
    assert_eq!(frames[0]["kind"], "udp");
    assert_eq!(frames[4]["summary"], "ARP who-has 10.0.2.2 tell 10.0.2.15");
    assert_eq!(frames[5]["direction"], "rx");
    assert!(
        frames[5]["hex"]
            .as_str()
            .unwrap()
            .starts_with("000a35001e53")
    );
    let t: Vec<u64> = frames.iter().map(|f| f["t_ns"].as_u64().unwrap()).collect();
    assert!(t.windows(2).all(|w| w[0] <= w[1]), "{t:?}");

    // Paging.
    let page = call(&mut s, "net_capture", json!({ "from": 10, "max": 10 }));
    let p = &page["structuredContent"];
    assert_eq!(p["frames"].as_array().unwrap().len(), 4);
    assert_eq!(p["frames"][0]["index"], 10);
    assert_eq!(p["next"], 14);

    // Injection is refused with an explanation, not silently dropped.
    let r = call(&mut s, "net_send", json!({ "hex": "ffffffffffff" }));
    assert_eq!(r["isError"], true, "{r}");
    let msg = r["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("user-mode"), "{msg}");

    // Snapshot, run on, restore: the digest and the capture come back.
    let hash0 =
        call(&mut s, "status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    let r = call(&mut s, "snapshot_save", json!({ "name": "pinged" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "sim_run", json!({ "duration": "2ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call(&mut s, "snapshot_restore", json!({ "name": "pinged" }));
    assert_eq!(r["isError"], false, "{r}");
    let st = call(&mut s, "status_get", json!({}));
    assert_eq!(st["structuredContent"]["status"]["trace_hash"], hash0);
    assert_eq!(st["structuredContent"]["network"]["frames_tx"], 7);
    let cap = call(&mut s, "net_capture", json!({}));
    assert_eq!(cap["structuredContent"]["total"], 14);

    // The whole session replays to the same digest.
    let rep = call(&mut s, "session_replay", json!({}));
    assert_eq!(rep["isError"], false, "{rep}");
    assert_eq!(rep["structuredContent"]["deterministic"], true, "{rep}");
}
