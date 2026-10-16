//! The P3 exit demonstration through MCP (ADR-019): `project_create_from_xsa` (with the BOOT.BIN),
//! `project_open`, `sim_run`. The design, firmware and BOOT.BIN are in `fixtures/handoff`.
//!
//! The end-to-end test needs the Xilinx QEMU, Vivado XSim and the DPI library and prints
//! `SKIPPED:` without QEMU or XSim.

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

fn out_dir(tag: &str) -> String {
    let rel = format!("target/handoff-mcp-{tag}-{}", std::process::id());
    let _ = std::fs::remove_dir_all(workspace().join(&rel));
    rel
}

fn found(s: &mut zynqsim_mcp::McpServer, backend: &str) -> bool {
    call(s, "backend_list", json!({}))["structuredContent"]["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == backend && b["found"] == true)
}

/// Creates the project from the handoff; with `export` the user RTL comes from the export.
fn create(s: &mut zynqsim_mcp::McpServer, tag: &str, export: bool) -> String {
    let out = out_dir(tag);
    let mut args = json!({
        "xsa_path": "fixtures/handoff/design.xsa",
        "out_dir": out,
        "boot_bin": "fixtures/handoff/BOOT.BIN",
    });
    if export {
        args["export_path"] = json!("fixtures/handoff/export/xsim");
    } else {
        args["rtl"] = json!(["fixtures/handoff/rtl/my_math_ip.v"]);
    }
    let r = call(s, "project_create_from_xsa", args);
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert_eq!(v["runnable"], true, "{v}");
    assert_eq!(v["errors"], 0, "{v}");
    assert_eq!(v["boot"]["mode"], "boot_bin", "{v}");
    assert_eq!(v["boot"]["file"], "BOOT.BIN", "{v}");
    assert!(v["unmodelled"].as_array().unwrap().is_empty(), "{v}");
    let classes: Vec<&str> = v["pl_instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["instance"].as_str().unwrap())
        .collect();
    for want in ["u_math", "axi_gpio_0", "axi_timer_0", "axi_uartlite_0"] {
        assert!(classes.contains(&want), "{want} in {classes:?}");
    }
    assert!(workspace().join(&out).join("BOOT.BIN").is_file());
    out
}

fn assert_firmware_passed(s: &mut zynqsim_mcp::McpServer) {
    let u1 = call(s, "uart_read", json!({ "port": 1 }));
    let text = u1["structuredContent"]["text"].as_str().unwrap();
    assert_eq!(text.matches("math PASS").count(), 7, "{text}");
    assert!(text.contains("timer ticks=3"), "{text}");
    assert!(text.contains("gpio: tri=0x0 data=0x9"), "{text}");
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
    let u2 = call(s, "uart_read", json!({ "port": 2 }));
    assert_eq!(u2["structuredContent"]["text"], "HANDOFF UART OK\r\n");
    let leds = call(s, "board_get_leds", json!({}))["structuredContent"]["leds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["on"].as_bool().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(leds, vec![true, false, false, true]);
}

/// Creating the project needs no external tool.
#[test]
fn the_handoff_project_is_created_with_the_bootbin_through_mcp() {
    let mut s = server();
    let out = create(&mut s, "create", false);
    let toml = std::fs::read_to_string(workspace().join(&out).join("project.toml")).unwrap();
    assert!(toml.contains("mode = \"boot_bin\""), "{toml}");
    assert!(!toml.contains("[cpu0]"), "{toml}");
    let out = create(&mut s, "create-export", true);
    let toml = std::fs::read_to_string(workspace().join(&out).join("project.toml")).unwrap();
    assert!(toml.contains("export_dir ="), "{toml}");
    // A damaged image creates nothing.
    let rel = out_dir("bad");
    let dir = workspace().join(&rel);
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = std::fs::read(workspace().join("fixtures/handoff/BOOT.BIN")).unwrap();
    bytes[0x44] ^= 1;
    std::fs::write(dir.join("bad.bin"), &bytes).unwrap();
    let r = call(
        &mut s,
        "project_create_from_xsa",
        json!({
            "xsa_path": "fixtures/handoff/design.xsa",
            "out_dir": format!("{rel}/p"),
            "boot_bin": format!("{rel}/bad.bin"),
        }),
    );
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("checksum"), "{r}");
    assert!(!dir.join("p/project.toml").exists());
}

/// `project_create_from_xsa` + `project_open` + `sim_run`: the firmware boots from the BOOT.BIN
/// and passes against the user RTL in XSim and the vendor models, for both variants.
#[test]
fn the_exit_design_runs_end_to_end_through_mcp() {
    let mut s = server();
    if !found(&mut s, "qemu") || !found(&mut s, "xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    assert!(
        workspace()
            .join("target/dpi/release/libxsim_dpi.a")
            .is_file(),
        "build the DPI library first: cargo xtask dpi"
    );
    let mut hashes = Vec::new();
    for (tag, export) in [("run-xsa", false), ("run-export", true)] {
        let out = create(&mut s, tag, export);
        let open = call(&mut s, "project_open", json!({ "path": out }));
        assert_eq!(open["isError"], false, "{open}");
        assert_eq!(open["structuredContent"]["simulation_started"], true);
        let st = call(&mut s, "status_get", json!({}));
        assert_eq!(
            st["structuredContent"]["status"]["pl_backend"], "xsim+native",
            "{st}"
        );
        let run = call(&mut s, "sim_run", json!({ "duration": "8ms" }));
        assert_eq!(run["isError"], false, "{run}");
        assert_firmware_passed(&mut s);
        let st = call(&mut s, "status_get", json!({}));
        hashes.push(st["structuredContent"]["status"]["trace_hash"].clone());
    }
    assert!(hashes[0].is_string(), "{hashes:?}");
    assert_eq!(
        hashes[0], hashes[1],
        "PL from the XSA and from the export: one trace digest"
    );
}
