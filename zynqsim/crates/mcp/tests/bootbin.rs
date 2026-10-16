//! `boot_inspect` and a project booted from a BOOT.BIN, through MCP (ADR-018).
//!
//! The inspection needs no external tool; the boot needs the Xilinx QEMU and prints
//! `SKIPPED:` without it.

use serde_json::{Value, json};
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn server() -> zynqsim_mcp::McpServer {
    zynqsim_mcp::McpServer::new(vec![workspace()]).unwrap()
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

#[test]
fn boot_inspect_lists_partitions_and_findings() {
    let mut s = server();
    let r = call(
        &mut s,
        "boot_inspect",
        json!({ "path": "fixtures/boot/BOOT.BIN" }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["runnable"], true);
    assert_eq!(v["entry"], "0x00100000");
    let parts = v["partitions"].as_array().unwrap();
    let kinds: Vec<&str> = parts.iter().map(|p| p["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["fsbl", "bitstream", "application", "application"]);
    assert_eq!(v["register_init"].as_array().unwrap().len(), 4);
    assert_eq!(v["fsbl"]["executed"], false);
    assert!(
        v["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("never applied"))
    );

    // A damaged copy: the finding names the field and the offset; nothing is run.
    let rel = format!("target/bootbin-mcp-{}", std::process::id());
    let dir = workspace().join(&rel);
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = std::fs::read(workspace().join("fixtures/boot/BOOT.BIN")).unwrap();
    bytes[0x44] ^= 1;
    std::fs::write(dir.join("bad.bin"), &bytes).unwrap();
    let r = call(
        &mut s,
        "boot_inspect",
        json!({ "path": format!("{rel}/bad.bin") }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["runnable"], false);
    assert_eq!(v["errors"][0]["field"], "boot_header.checksum");
    assert_eq!(v["errors"][0]["offset"], 0x48);

    // Not a boot image, and outside the roots.
    let r = call(
        &mut s,
        "boot_inspect",
        json!({ "path": "fixtures/elf/rust_hello.elf" }),
    );
    assert_eq!(r["isError"], true);
    assert!(r.to_string().contains("not a Zynq BootROM image"), "{r}");
    let r = call(
        &mut s,
        "boot_inspect",
        json!({ "path": "C:/Windows/notepad.exe" }),
    );
    assert_eq!(r["isError"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bootbin_project_boots_through_mcp() {
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
    let rel = format!("target/bootbin-mcp-run-{}", std::process::id());
    let dir = workspace().join(&rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        workspace().join("fixtures/boot/BOOT.BIN"),
        dir.join("BOOT.BIN"),
    )
    .unwrap();
    std::fs::write(
        dir.join("project.toml"),
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n\
         [backends]\nps = \"qemu\"\npl = \"demo\"\n\
         [boot]\nmode = \"bootbin\"\nfile = \"BOOT.BIN\"\n",
    )
    .unwrap();
    let open = call(&mut s, "project_open", json!({ "path": rel }));
    assert_eq!(open["isError"], false, "{open}");
    let run = call(&mut s, "sim_run", json!({ "duration": "5ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let r = call(&mut s, "uart_read", json!({ "port": 1 }));
    let text = r["structuredContent"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("RESULT: ALL PASS"), "{text:?}");
    assert!(text.contains("sum(1..=100) = 5050"), "{text:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
