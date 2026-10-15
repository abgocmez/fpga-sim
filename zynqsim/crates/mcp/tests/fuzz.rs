//! Seeded fuzzing of the MCP surface. This is a property test (never panics,
//! always answers well-formed JSON-RPC, never escapes the roots), not a
//! replacement for coverage-guided `cargo fuzz` (listed in docs/TESTPLAN.md).

mod common;

use common::Harness;
use serde_json::{Value, json};
use sim_core::Prng;
use zynqsim_mcp::{NoControl, tool_defs};

fn random_value(p: &mut Prng, depth: u8) -> Value {
    match p.range(0, 12) {
        0 => Value::Null,
        1 => json!(p.next_u64() % 2 == 0),
        2 => json!(p.range(0, 70)),
        3 => json!(-(p.range(1, 1000) as i64)),
        4 => json!(p.next_u64()),
        5 => json!(p.next_u64() as f64 / 7.0),
        6 => json!(
            [
                "10ms", "1s", "250us", "0ps", "5", "-1s", "1e999s", "99999s", ""
            ][p.range(0, 9) as usize]
        ),
        7 => json!(
            [
                "sw0",
                "btn3",
                "led5_g",
                "JA1",
                "nope",
                "../../etc/passwd",
                "C:\\Windows\\win.ini"
            ][p.range(0, 7) as usize]
        ),
        8 => json!("x".repeat(p.range(0, 2000) as usize)),
        9 => json!(["de ad", "zz", "abc", "0102"][p.range(0, 4) as usize]),
        10 if depth < 2 => Value::Array(
            (0..p.range(0, 4))
                .map(|_| random_value(p, depth + 1))
                .collect(),
        ),
        _ if depth < 2 => {
            let mut m = serde_json::Map::new();
            for _ in 0..p.range(0, 4) {
                m.insert(format!("k{}", p.range(0, 5)), random_value(p, depth + 1));
            }
            Value::Object(m)
        }
        _ => json!(1),
    }
}

fn arguments_for(p: &mut Prng, props: &[String]) -> Value {
    // Mix well-named keys with random values and pure noise.
    match p.range(0, 5) {
        0 => random_value(p, 0),
        1 => Value::Null,
        _ => {
            let mut m = serde_json::Map::new();
            for k in props {
                if p.range(0, 3) != 0 {
                    m.insert(k.clone(), random_value(p, 1));
                }
            }
            if p.range(0, 6) == 0 {
                m.insert("zzz".into(), random_value(p, 1));
            }
            Value::Object(m)
        }
    }
}

#[test]
fn random_tool_calls_never_panic_and_always_answer_well_formed_json_rpc() {
    let mut h = Harness::new("fuzz");
    let mut p = Prng::new(0x5EED);
    let defs = tool_defs();
    let props: Vec<Vec<String>> = defs
        .iter()
        .map(|d| {
            d.schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        })
        .collect();

    let mut errors = 0;
    let mut successes = 0;
    for i in 0..1500 {
        let t = p.range(0, defs.len() as u64) as usize;
        let args = arguments_for(&mut p, &props[t]);
        let resp = h
            .server
            .handle(
                json!({ "jsonrpc": "2.0", "id": i, "method": "tools/call",
                        "params": { "name": defs[t].name, "arguments": args } }),
                &mut NoControl,
            )
            .expect("a request with an id is always answered");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], i);
        let result = resp.get("result").unwrap_or_else(|| {
            panic!(
                "tool {} with args produced a protocol error: {resp}",
                defs[t].name
            )
        });
        assert!(result["isError"].is_boolean());
        assert!(result["content"].as_array().is_some_and(|c| !c.is_empty()));
        if result["isError"] == true {
            errors += 1;
            let code = result["structuredContent"]["error"]["code"]
                .as_str()
                .unwrap();
            assert!(
                [
                    "invalid_argument",
                    "not_found",
                    "unsupported",
                    "io",
                    "cancelled"
                ]
                .contains(&code),
                "unexpected error code {code}: {result}"
            );
        } else {
            successes += 1;
        }
    }
    // Both outcomes must occur or the generator is not exercising anything.
    assert!(
        errors > 100 && successes > 100,
        "errors={errors} successes={successes}"
    );
}

#[test]
fn random_garbage_requests_never_panic() {
    let mut h = Harness::new("garbage");
    let mut p = Prng::new(77);
    for _ in 0..500 {
        let msg = match p.range(0, 4) {
            0 => random_value(&mut p, 0),
            1 => {
                json!({ "jsonrpc": "2.0", "id": random_value(&mut p, 1), "method": random_value(&mut p, 1) })
            }
            2 => {
                json!({ "jsonrpc": "2.0", "method": "tools/call", "params": random_value(&mut p, 0) })
            }
            _ => {
                json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/read", "params": random_value(&mut p, 0) })
            }
        };
        // Any outcome (None, result or error) is acceptable; panicking is not.
        let _ = h.server.handle(msg, &mut NoControl);
    }
}
