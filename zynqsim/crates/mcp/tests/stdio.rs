//! Tests of the stdio transport: the real binary, parse errors, and
//! progress/cancellation of a long run.

mod common;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Cursor, Write};
use std::process::{Command, Stdio};
use zynqsim_mcp::{McpServer, serve};

#[test]
fn real_binary_speaks_mcp_over_pipes() {
    let root = common::temp_dir("bin");
    let mut child = Command::new(env!("CARGO_BIN_EXE_zynqsim-mcp"))
        .arg("--root")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());

    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = || -> Value {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}"))
    };

    send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                 "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                             "clientInfo": { "name": "test", "version": "0" } } }));
    assert_eq!(recv()["result"]["serverInfo"]["name"], "zynqsim-mcp");
    send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                 "params": { "name": "sim_run", "arguments": { "duration": "1ms" } } }));
    let r = recv();
    assert_eq!(r["id"], 2, "notification must not produce a reply: {r}");
    assert_eq!(r["result"]["structuredContent"]["now_ps"], 1_000_000_000u64);

    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn bad_input_lines_get_json_rpc_errors_and_the_server_keeps_going() {
    let root = common::temp_dir("bad");
    let mut server = McpServer::new(vec![root]).unwrap();
    let input = "not json at all\n[1,2]\n\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n";
    let mut out = Vec::new();
    serve(&mut server, Cursor::new(input.to_string()), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["error"]["code"], -32600);
    assert_eq!(lines[2]["id"], 3);
}

#[test]
fn long_run_reports_progress_and_can_be_cancelled() {
    let root = common::temp_dir("cancel");
    let mut server = McpServer::new(vec![root]).unwrap();
    let input = [
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": "sim_run", "arguments": { "duration": "3600s" },
                            "_meta": { "progressToken": "tok" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": { "requestId": 7 } }),
        json!({ "jsonrpc": "2.0", "id": 8, "method": "ping" }),
    ]
    .map(|v| v.to_string())
    .join("\n")
        + "\n";

    let mut out = Vec::new();
    serve(&mut server, Cursor::new(input), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let progress: Vec<&Value> = lines
        .iter()
        .filter(|l| l["method"] == "notifications/progress")
        .collect();
    for p in &progress {
        assert_eq!(p["params"]["progressToken"], "tok");
        assert_eq!(p["params"]["total"], 3600.0);
    }
    let reply = lines
        .iter()
        .find(|l| l["id"] == 7)
        .expect("reply to request 7");
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(
        reply["result"]["structuredContent"]["error"]["code"],
        "cancelled"
    );
    // The request after the cancel is still served (it was not swallowed).
    assert!(
        lines
            .iter()
            .any(|l| l["id"] == 8 && l["result"] == json!({}))
    );
    // And the server stopped early: fewer progress steps than the 3600 chunks.
    assert!(
        progress.len() < 3600,
        "ran to completion despite cancellation"
    );
}
