#![allow(dead_code, missing_docs)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use zynqsim_mcp::{McpServer, NoControl};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, empty directory under the system temp dir.
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("zynqsim-mcp-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub const PROJECT: &str = r#"
schema_version = 1
[board]
name = "zybo-z7-20"
[sim]
seed = 5
"#;

/// A server rooted at a temp dir that already holds `p/project.toml`.
pub struct Harness {
    pub server: McpServer,
    pub root: PathBuf,
    next_id: u64,
}

impl Harness {
    pub fn new(tag: &str) -> Harness {
        let root = temp_dir(tag);
        std::fs::create_dir_all(root.join("p")).unwrap();
        std::fs::write(root.join("p").join("project.toml"), PROJECT).unwrap();
        let server = McpServer::new(vec![root.clone()]).unwrap();
        Harness {
            server,
            root,
            next_id: 1,
        }
    }

    /// Sends a request and returns the full JSON-RPC response object.
    pub fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.server
            .handle(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                &mut NoControl,
            )
            .expect("requests with an id get a response")
    }

    /// Calls a tool and returns the `result` object (panics on a JSON-RPC error).
    pub fn call(&mut self, name: &str, args: Value) -> Value {
        let r = self.rpc("tools/call", json!({ "name": name, "arguments": args }));
        r.get("result")
            .unwrap_or_else(|| panic!("{name}: JSON-RPC error {r}"))
            .clone()
    }

    /// Calls a tool that must succeed and returns its structured content.
    pub fn ok(&mut self, name: &str, args: Value) -> Value {
        let r = self.call(name, args);
        assert_eq!(r["isError"], false, "{name} failed: {r}");
        r["structuredContent"].clone()
    }

    /// Calls a tool that must fail and returns `error.code`, asserting the message.
    pub fn err(&mut self, name: &str, args: Value, message_contains: &str) -> String {
        let r = self.call(name, args);
        assert_eq!(r["isError"], true, "{name} should have failed: {r}");
        let e = &r["structuredContent"]["error"];
        let msg = e["message"].as_str().unwrap();
        assert!(
            msg.contains(message_contains),
            "{name}: `{msg}` lacks `{message_contains}`"
        );
        e["code"].as_str().unwrap().to_string()
    }
}
