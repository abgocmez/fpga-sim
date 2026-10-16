//! Seeded fuzzing of every MCP tool and of the raw JSON-RPC line transport
//! (spec §14.8 and §14.10). Deterministic and stable-Rust only; the fixed
//! seeds make every failure reproducible. The coverage-guided counterpart is
//! `fuzz/` (cargo-fuzz, not part of the workspace).
//!
//! Each request runs on a worker thread that owns the server. The test thread
//! waits at most 5 s for an answer, so a hang fails the test instead of
//! blocking it, and a panic is caught and reported with the input.

mod common;

use common::temp_dir;
use serde_json::{Value, json};
use sim_core::Prng;
use std::io::Cursor;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use zynqsim_mcp::{McpServer, NoControl, serve, tool_defs};

/// Random argument objects per tool and scenario.
const CALLS_PER_TOOL: usize = 200;
/// Wall-clock limit for one request.
const LIMIT: Duration = Duration::from_secs(5);
/// Limit for rendering tools (see `limit_for`).
const RENDER_LIMIT: Duration = Duration::from_secs(60);
const KNOWN_ERROR_CODES: &[&str] = &[
    "invalid_argument",
    "not_found",
    "unsupported",
    "io",
    "disconnected",
    "cancelled",
    "internal",
];

/// What the worker sends back for one request.
enum Reply {
    Answered(Option<Value>, Duration),
    Panicked,
}

/// A server on its own thread, so each request can be given a deadline.
struct Worker {
    tx: Sender<Value>,
    rx: Receiver<Reply>,
}

impl Worker {
    fn spawn(mut server: McpServer) -> Worker {
        let (tx, req_rx) = mpsc::channel::<Value>();
        let (rep_tx, rx) = mpsc::channel::<Reply>();
        std::thread::spawn(move || {
            for msg in req_rx {
                let t = Instant::now();
                let out =
                    panic::catch_unwind(AssertUnwindSafe(|| server.handle(msg, &mut NoControl)));
                let reply = match out {
                    Ok(v) => Reply::Answered(v, t.elapsed()),
                    Err(_) => Reply::Panicked,
                };
                if rep_tx.send(reply).is_err() {
                    break;
                }
            }
        });
        Worker { tx, rx }
    }

    /// Sends one message and returns the answer; fails on a panic or a hang.
    fn ask(&self, msg: Value, limit: Duration) -> Option<Value> {
        self.tx.send(msg.clone()).expect("worker alive");
        match self.rx.recv_timeout(limit) {
            Ok(Reply::Answered(v, took)) => {
                assert!(took < limit, "request took {took:?}: {msg}");
                v
            }
            Ok(Reply::Panicked) => panic!("PANIC on input:\n{msg}"),
            Err(e) => panic!("no answer within {limit:?} ({e}) for input:\n{msg}"),
        }
    }
}

/// The wall-clock limit for one call to `tool`. Rendering tools get more room:
/// an unoptimised (debug) build takes about 8 s for `board_screenshot` at
/// scale 4, which is a speed of the test profile, not a hang.
fn limit_for(tool: &str) -> Duration {
    if tool.contains("screenshot") || tool.contains("render") {
        RENDER_LIMIT
    } else {
        LIMIT
    }
}

/// Fresh root with `p/project.toml` (a copy of `examples/demo`) and a server
/// rooted there, so the fuzzer can never write outside the temp directory.
fn demo_root(tag: &str) -> PathBuf {
    let root = temp_dir(tag);
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo/project.toml");
    std::fs::create_dir_all(root.join("p")).unwrap();
    std::fs::copy(&demo, root.join("p").join("project.toml")).unwrap();
    root
}

/// `tools/list` through the protocol: `(name, inputSchema)` for every tool.
fn list_tools(w: &Worker) -> Vec<(String, Value)> {
    let resp = w
        .ask(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
            LIMIT,
        )
        .expect("tools/list is answered");
    resp["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_string(),
                t["inputSchema"].clone(),
            )
        })
        .collect()
}

/// A random JSON value of any shape, including the kinds a schema forbids.
fn random_value(p: &mut Prng, depth: u8) -> Value {
    match p.range(0, 13) {
        0 => Value::Null,
        1 => json!(p.next_u64() % 2 == 0),
        2 => json!(p.range(0, 70)),
        3 => json!(-(p.range(1, 1_000_000) as i64)),
        4 => json!(p.next_u64()),
        5 => json!(p.next_u64() as f64 / 7.0),
        6 => json!(
            [
                "10ms",
                "1s",
                "250us",
                "0ps",
                "5",
                "-1s",
                "1e999s",
                "99999999999999999999s",
                "",
                "NaN"
            ][p.range(0, 10) as usize]
        ),
        7 => json!(
            [
                "sw0",
                "btn3",
                "led5_g",
                "JA1",
                "nope",
                "../../etc/passwd",
                "C:\\Windows\\win.ini",
                "p",
                "p/project.toml",
                "\u{0}",
                "é😀\u{202e}"
            ][p.range(0, 11) as usize]
        ),
        8 => json!("x".repeat(p.range(0, 20_000) as usize)),
        9 => json!(["de ad", "zz", "abc", "0102", "0x", "-0"][p.range(0, 6) as usize]),
        10 if depth < 3 => Value::Array(
            (0..p.range(0, 5))
                .map(|_| random_value(p, depth + 1))
                .collect(),
        ),
        11 if depth < 3 => {
            let mut m = serde_json::Map::new();
            for _ in 0..p.range(0, 5) {
                m.insert(format!("k{}", p.range(0, 6)), random_value(p, depth + 1));
            }
            Value::Object(m)
        }
        _ => json!(u64::MAX),
    }
}

/// A value shaped by the property schema, sometimes deliberately wrong.
fn value_for(p: &mut Prng, prop: &Value) -> Value {
    if let Some(choices) = prop.get("enum").and_then(Value::as_array) {
        if !choices.is_empty() && p.range(0, 4) != 0 {
            return choices[p.range(0, choices.len() as u64) as usize].clone();
        }
    }
    match prop.get("type").and_then(Value::as_str) {
        Some("string") if p.range(0, 3) != 0 => {
            json!(["p", "demo", "x", "1", ""][p.range(0, 5) as usize])
        }
        Some("integer") if p.range(0, 3) != 0 => json!(p.range(0, 100)),
        Some("number") if p.range(0, 3) != 0 => json!(p.range(0, 100) as f64 / 3.0),
        Some("boolean") if p.range(0, 3) != 0 => json!(p.range(0, 2) == 0),
        _ => random_value(p, 1),
    }
}

/// Arguments for one tool: sometimes not an object at all, otherwise the
/// schema's keys with a mix of plausible, wrong and missing values and extra keys.
fn random_arguments(p: &mut Prng, schema: &Value) -> Value {
    match p.range(0, 8) {
        0 => random_value(p, 0),
        1 => Value::Null,
        2 => json!({}),
        _ => {
            let mut m = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, prop) in props {
                    // Dropping keys exercises "missing required".
                    if p.range(0, 4) != 0 {
                        let v = if p.range(0, 2) == 0 {
                            value_for(p, prop)
                        } else {
                            random_value(p, 1)
                        };
                        m.insert(k.clone(), v);
                    }
                }
            }
            if p.range(0, 3) == 0 {
                m.insert(format!("extra{}", p.range(0, 4)), random_value(p, 1));
            }
            if p.range(0, 20) == 0 {
                m.insert("x".repeat(p.range(1, 3000) as usize), json!(1));
            }
            Value::Object(m)
        }
    }
}

/// Every answer to a `tools/call` is a JSON-RPC result with a boolean `isError`,
/// or a JSON-RPC error. Failed results carry a known error code.
fn check_tool_answer(name: &str, answer: Option<Value>, input: &Value) -> bool {
    let resp = answer.unwrap_or_else(|| panic!("{name}: no response to a request with an id"));
    assert_eq!(resp["jsonrpc"], "2.0", "{name}: {resp}");
    if let Some(err) = resp.get("error") {
        assert!(err["code"].is_i64(), "{name}: error without a code: {resp}");
        assert!(
            err["message"].is_string(),
            "{name}: error without a message: {resp}"
        );
        return false;
    }
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("{name}: neither result nor error: {resp}\ninput: {input}"));
    assert!(result["isError"].is_boolean(), "{name}: {resp}");
    assert!(
        result["content"].as_array().is_some_and(|c| !c.is_empty()),
        "{name}: {resp}"
    );
    if result["isError"] == true {
        let code = result["structuredContent"]["error"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: error result without a code: {resp}"));
        assert!(
            KNOWN_ERROR_CODES.contains(&code),
            "{name}: unexpected error code {code}: {resp}"
        );
        false
    } else {
        true
    }
}

fn fuzz_every_tool(tag: &str, root: PathBuf, open_demo: bool, seed: u64) {
    let server = McpServer::new(vec![root]).expect("server");
    let w = Worker::spawn(server);
    if open_demo {
        let r = w
            .ask(
                json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/call",
                         "params": { "name": "project_open", "arguments": { "path": "p" } } }),
                LIMIT,
            )
            .expect("project_open answered");
        assert_eq!(
            r["result"]["isError"], false,
            "{tag}: demo did not open: {r}"
        );
    }
    let tools = list_tools(&w);
    assert_eq!(tools.len(), tool_defs().len(), "tools/list is incomplete");

    let mut p = Prng::new(seed);
    let mut ok_results = 0usize;
    let mut error_results = 0usize;
    let mut protocol_errors = 0usize;
    for (name, schema) in &tools {
        for i in 0..CALLS_PER_TOOL {
            let args = random_arguments(&mut p, schema);
            let input = json!({ "jsonrpc": "2.0", "id": i, "method": "tools/call",
                                "params": { "name": name, "arguments": args } });
            let answer = w.ask(input.clone(), limit_for(name));
            if check_tool_answer(name, answer.clone(), &input) {
                ok_results += 1;
            } else {
                match answer.as_ref().and_then(|a| a.get("result")) {
                    Some(_) => error_results += 1,
                    None => protocol_errors += 1,
                }
            }
        }
    }
    // Both kinds of result must occur or the generator is not exercising the tools.
    assert!(
        ok_results > 500 && error_results > 500,
        "{tag}: ok={ok_results} error={error_results} protocol={protocol_errors}"
    );
}

#[test]
fn every_tool_survives_random_arguments_without_a_project() {
    let root = temp_dir("fuzz-req-none");
    fuzz_every_tool("no project", root, false, 0xA11CE);
}

#[test]
fn every_tool_survives_random_arguments_with_the_demo_open() {
    fuzz_every_tool("demo", demo_root("fuzz-req-demo"), true, 0xB0B);
}

/// Lines that are not JSON-RPC requests at all, or are requests for unknown
/// methods, with unusual ids. Each line gets zero or more well-formed replies.
#[test]
fn raw_json_rpc_lines_are_always_answered_or_ignored() {
    let root = demo_root("fuzz-raw");
    let mut p = Prng::new(0xC0FFEE);
    let mut lines: Vec<String> = vec![
        "{".into(),
        "not json at all".into(),
        "\u{0}\u{1}".into(),
        "null".into(),
        "42".into(),
        "\"just a string\"".into(),
        "[]".into(),
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":18446744073709551616,\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":1e308,\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":{\"a\":[1,2]},\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":42}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"no_such_tool\"}}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"resources/read\",\"params\":{\"uri\":\"zynqsim://\"}}".into(),
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":1}}".into(),
    ];
    // Random mutations of a valid request line.
    let base = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"status_get","arguments":{"x":1}}}"#;
    for _ in 0..600 {
        let mut b = base.as_bytes().to_vec();
        for _ in 0..1 + p.range(0, 4) {
            let i = p.range(0, b.len() as u64 + 1) as usize;
            match p.range(0, 4) {
                0 if !b.is_empty() => {
                    let j = i.min(b.len() - 1);
                    b[j] = p.next_u64() as u8;
                }
                1 => b.truncate(i),
                2 => b.insert(i.min(b.len()), b"{}[]\":,\\"[p.range(0, 8) as usize]),
                _ => {
                    let at = i.min(b.len());
                    b.splice(at..at, "\u{0}é\n".bytes());
                }
            }
        }
        lines.push(String::from_utf8_lossy(&b).into_owned());
    }
    // Random well-formed envelopes with arbitrary methods and ids.
    for _ in 0..300 {
        const METHODS: &[&str] = &[
            "ping",
            "tools/list",
            "tools/call",
            "resources/read",
            "prompts/get",
            "x",
            "",
        ];
        let version = if p.range(0, 4) == 0 {
            json!(2)
        } else {
            json!("2.0")
        };
        let method = METHODS[p.range(0, METHODS.len() as u64) as usize];
        let msg = json!({
            "jsonrpc": version,
            "id": random_value(&mut p, 1),
            "method": method,
            "params": random_value(&mut p, 0),
        });
        lines.push(msg.to_string());
    }

    let input: Vec<u8> = lines
        .iter()
        .flat_map(|l| l.bytes().chain(std::iter::once(b'\n')))
        .collect();
    let mut server = McpServer::new(vec![root]).expect("server");
    let mut out = Vec::new();
    let transport = panic::catch_unwind(AssertUnwindSafe(|| {
        serve(&mut server, Cursor::new(input), &mut out)
    }));
    let transport = transport.unwrap_or_else(|_| panic!("serve() panicked"));
    transport.expect("the transport itself does not fail on in-memory input");

    let text = String::from_utf8(out).expect("replies are UTF-8");
    let mut answered = 0usize;
    for reply in text.lines() {
        let v: Value = serde_json::from_str(reply)
            .unwrap_or_else(|e| panic!("reply is not JSON ({e}): {reply}"));
        assert_eq!(v["jsonrpc"], "2.0", "{reply}");
        assert!(
            v.get("result").is_some() ^ v.get("error").is_some() || v.get("method").is_some(),
            "reply is neither a result, an error nor a notification: {reply}"
        );
        answered += 1;
    }
    // Invalid JSON must be reported as a parse error, not dropped silently.
    assert!(
        text.contains("-32700"),
        "no parse error reply among {answered} replies"
    );
}
