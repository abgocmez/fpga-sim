#![no_main]
// Entry point: one JSON-RPC message -> McpServer::handle (no project open).
// Same properties as crates/mcp/tests/fuzz_requests.rs: no panic, always a
// well-formed answer or none. File access is confined to a scratch root.

use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;
use std::path::PathBuf;
use zynqsim_mcp::{McpServer, NoControl};

thread_local! {
    static SERVER: RefCell<Option<McpServer>> = const { RefCell::new(None) };
}

fn scratch_root() -> PathBuf {
    let root = std::env::temp_dir().join("zynqsim-fuzz-root");
    std::fs::create_dir_all(&root).expect("scratch root");
    root
}

fuzz_target!(|data: &[u8]| {
    let Ok(msg) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    SERVER.with(|s| {
        let mut slot = s.borrow_mut();
        let server =
            slot.get_or_insert_with(|| McpServer::new(vec![scratch_root()]).expect("server"));
        if let Some(answer) = server.handle(msg, &mut NoControl) {
            assert_eq!(answer["jsonrpc"], "2.0");
        }
    });
});
