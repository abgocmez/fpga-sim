//! In-repo MCP client scenario tests (prompt §13/§14.8).

mod common;

use base64::Engine as _;
use common::Harness;
use serde_json::json;
use std::collections::HashSet;
use zynqsim_mcp::tool_defs;

#[test]
fn handshake_lists_every_tool_resource_and_prompt() {
    let mut h = Harness::new("hs");
    let init = h.rpc("initialize", json!({ "protocolVersion": "2025-06-18" }));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "zynqsim-mcp");
    // An unknown version is answered with one the server speaks.
    let init = h.rpc("initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let tools = h.rpc("tools/list", json!({}));
    let names: HashSet<String> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), tool_defs().len());
    assert!(names.contains("sim_run") && names.contains("board_screenshot"));

    let res = h.rpc("resources/list", json!({}));
    assert!(res["result"]["resources"].as_array().unwrap().len() >= 5);
    let st = h.rpc("resources/read", json!({ "uri": "zynqsim://docs/status" }));
    assert!(
        st["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ZynqSim feature status")
    );
    let missing = h.rpc("resources/read", json!({ "uri": "zynqsim://nope" }));
    assert_eq!(missing["error"]["code"], -32002);

    let prompts = h.rpc("prompts/list", json!({}));
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 2);
    let p = h.rpc(
        "prompts/get",
        json!({ "name": "debug_a_hang", "arguments": { "symptom": "no UART" } }),
    );
    assert!(
        p["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("no UART")
    );
    let bad = h.rpc("prompts/get", json!({ "name": "explain_axi_violation" }));
    assert_eq!(bad["error"]["code"], -32602);

    assert_eq!(h.rpc("no/such/method", json!({}))["error"]["code"], -32601);
    assert_eq!(h.rpc("ping", json!({}))["result"], json!({}));
}

#[test]
fn load_run_press_button_read_uart_screenshot_trace_signal() {
    let mut h = Harness::new("scn");

    // Load.
    let open = h.ok("project_open", json!({ "path": "p" }));
    assert_eq!(open["simulation_started"], true);
    assert_eq!(open["errors"], 0);

    // The status must say what is and is not simulated.
    let st = h.ok("status_get", json!({}));
    assert_eq!(st["status"]["fidelity"], "demo");
    assert_eq!(st["status"]["seed"], 5);
    assert_eq!(st["status"]["ps_backend"], "demo");

    // Run, then read the banner. Reads are cursor-based.
    h.ok("sim_run", json!({ "duration": "5ms" }));
    let u = h.ok("uart_read", json!({ "port": 1 }));
    assert_eq!(u["text"], "zynqsim demo ready\r\n");
    let again = h.ok("uart_read", json!({ "port": 1 }));
    assert_eq!(again["bytes"], 0, "cursor must not re-deliver bytes");
    let replay = h.ok("uart_read", json!({ "port": 1, "from": 0, "max": 7 }));
    assert_eq!(replay["text"], "zynqsim");

    // Switch and LEDs.
    h.ok("board_set_switch", json!({ "id": "sw1", "on": true }));
    let leds = h.ok("board_get_leds", json!({}));
    assert_eq!(leds["leds"][1]["on"], true);
    assert_eq!(leds["leds"][0]["on"], false);
    assert_eq!(h.ok("pin_get", json!({ "name": "led1" }))["level"], "1");

    // Press a button and read the reaction.
    let tap = h.ok("board_press_button", json!({ "index": 0, "hold": "1ms" }));
    assert_eq!(tap["button"], "btn0");
    h.ok("sim_run", json!({ "duration": "5ms" }));
    assert_eq!(
        h.ok("uart_read", json!({ "port": 1 }))["text"],
        "BTN0 pressed\r\n"
    );

    // Echo through the target.
    h.ok("uart_write", json!({ "port": 1, "text": "ok" }));
    h.ok("sim_run", json!({ "duration": "5ms" }));
    assert_eq!(h.ok("uart_read", json!({ "port": 1 }))["text"], "ok");

    // Screenshot is a real PNG image content item.
    let r = h.call("board_screenshot", json!({ "scale": 0.5 }));
    let img = &r["content"][1];
    assert_eq!(img["type"], "image");
    assert_eq!(img["mimeType"], "image/png");
    let png = base64::engine::general_purpose::STANDARD
        .decode(img["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

    // Trace a signal: the waveform contains the LED and the UART byte stream.
    let w = h.ok("waveform_export", json!({ "format": "vcd" }));
    let vcd = w["text"].as_str().unwrap();
    assert!(vcd.contains("board.led1") && vcd.contains("uart1.tx") && vcd.contains("board.btn0"));
    let page = h.ok("trace_get", json!({ "from": 0, "max": 5 }));
    assert_eq!(page["records"].as_array().unwrap().len(), 5);
    assert!(page["total"].as_u64().unwrap() > 5);

    // The session replays to the same digest.
    let rep = h.ok("session_replay", json!({}));
    assert_eq!(rep["deterministic"], true, "{rep}");
}

#[test]
fn snapshot_reset_and_config() {
    let mut h = Harness::new("snap");
    h.ok("sim_run", json!({ "duration": "2ms" }));
    let before = h.ok("status_get", json!({}))["status"]["now_ps"].clone();
    h.ok("snapshot_save", json!({ "name": "a" }));
    h.ok("sim_run", json!({ "duration": "1s" }));
    let r = h.ok("snapshot_restore", json!({ "name": "a" }));
    assert_eq!(r["now_ps"], before);
    assert_eq!(
        h.err("snapshot_restore", json!({ "name": "zz" }), "no snapshot"),
        "not_found"
    );

    h.ok("config_set", json!({ "quantum": "5us" }));
    assert_eq!(
        h.ok("status_get", json!({}))["status"]["quantum_ps"],
        5_000_000
    );
    h.ok("sim_step", json!({}));
    assert_eq!(
        h.ok("status_get", json!({}))["status"]["now_ps"]
            .as_u64()
            .unwrap(),
        before.as_u64().unwrap() + 5_000_000
    );
    h.ok("config_set", json!({ "seed": 99 }));
    assert_eq!(h.ok("status_get", json!({}))["status"]["seed"], 99);
    h.err("config_set", json!({}), "needs `quantum` and/or `seed`");

    h.ok("sim_run_until", json!({ "time": "10s" }));
    assert_eq!(
        h.ok("sim_pause", json!({}))["now_ps"],
        10_000_000_000_000u64
    );
}

#[test]
fn unsupported_tools_say_why_instead_of_guessing() {
    let mut h = Harness::new("uns");
    let code = h.err("cpu_regs", json!({ "core": 0 }), "`CpuState` capability");
    assert_eq!(code, "unsupported");
    h.err("mem_read", json!({ "addr": 0, "len": 4 }), "MemoryAccess");
    h.err("pl_axi_log", json!({}), "PlSignals");
    h.err("video_frame", json!({}), "Video");
    h.err("net_capture", json!({}), "Network");
    let code = h.err(
        "pmod_attach",
        json!({ "port": "JA", "module": "oled" }),
        "`Pmod` capability",
    );
    assert_eq!(code, "unsupported");
    h.err(
        "project_create_from_xsa",
        json!({ "xsa_path": "x.xsa" }),
        "milestone P3",
    );
    // The message names the backend and fidelity so the caller knows the scope.
    let r = h.call("cpu_regs", json!({}));
    let msg = r["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("ps=`demo`") && msg.contains("Demo"), "{msg}");
}

#[test]
fn argument_validation_gives_precise_errors() {
    let mut h = Harness::new("val");
    h.err(
        "sim_run",
        json!({}),
        "missing required parameter `duration`",
    );
    h.err(
        "sim_run",
        json!({ "duration": 5 }),
        "`duration` must be string",
    );
    h.err("sim_run", json!({ "duration": "5" }), "missing unit");
    h.err("sim_run", json!({ "duration": "2h" }), "unknown unit");
    h.err("sim_run", json!({ "duration": "7200s" }), "at most 3600 s");
    h.err(
        "sim_run",
        json!({ "duration": "1ms", "extra": 1 }),
        "unknown parameter `extra`",
    );
    h.err("board_set_switch", json!({ "on": true }), "exactly one of");
    h.err(
        "board_set_switch",
        json!({ "index": 0, "id": "sw0", "on": true }),
        "exactly one of",
    );
    h.err(
        "board_set_switch",
        json!({ "index": 9, "on": true }),
        "out of range",
    );
    h.err(
        "board_set_switch",
        json!({ "id": "sw9", "on": true }),
        "no switch with id",
    );
    h.err(
        "uart_write",
        json!({ "port": 1 }),
        "exactly one of `text` or `hex`",
    );
    h.err(
        "uart_write",
        json!({ "port": 1, "text": "a", "hex": "61" }),
        "exactly one of",
    );
    h.err("uart_write", json!({ "port": 1, "hex": "6" }), "odd number");
    h.err("uart_write", json!({ "port": 2, "text": "a" }), "maximum");
    h.err("pin_get", json!({ "name": "nope" }), "no pin or control");
    h.err(
        "waveform_export",
        json!({ "format": "fst" }),
        "must be one of: vcd, perfetto",
    );
    // Unknown tool is a protocol error, not a tool result.
    let r = h.rpc("tools/call", json!({ "name": "rm_rf", "arguments": {} }));
    assert_eq!(r["error"]["code"], -32602);
}

#[test]
fn file_access_is_confined_to_the_allowed_roots() {
    let mut h = Harness::new("fs");
    let outside = common::temp_dir("outside");
    std::fs::write(outside.join("project.toml"), common::PROJECT).unwrap();

    for bad in [
        outside.join("project.toml").display().to_string(),
        "../outside".to_string(),
        "p/../../".to_string(),
    ] {
        let r = h.call("project_open", json!({ "path": bad }));
        assert_eq!(r["isError"], true, "{bad} must be refused: {r}");
    }
    h.err(
        "waveform_export",
        json!({ "format": "vcd", "path": "../escape.vcd" }),
        "outside the allowed roots",
    );
    assert!(!h.root.parent().unwrap().join("escape.vcd").exists());
    h.err("session_replay", json!({ "path": "../nope.json" }), "");

    // Inside the root works, and the file is really written.
    let w = h.ok(
        "waveform_export",
        json!({ "format": "perfetto", "path": "p/out.json" }),
    );
    assert!(w["bytes"].as_u64().unwrap() > 0);
    let text = std::fs::read_to_string(h.root.join("p").join("out.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&text).unwrap();
}

#[test]
fn project_diagnostics_do_not_replace_a_running_simulation() {
    let mut h = Harness::new("diag");
    h.ok("sim_run", json!({ "duration": "1ms" }));
    std::fs::write(
        h.root.join("p").join("bad.toml"),
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"native\"\n",
    )
    .unwrap();
    let open = h.ok("project_open", json!({ "path": "p/bad.toml" }));
    assert_eq!(open["simulation_started"], false);
    assert_eq!(open["errors"], 2); // backend not implemented + cpu0.elf required
    let fields: Vec<&str> = open["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"backends.ps") && fields.contains(&"cpu0.elf"),
        "{fields:?}"
    );
    // Time was not reset.
    assert_eq!(
        h.ok("status_get", json!({}))["status"]["now_ps"],
        1_000_000_000u64
    );

    let v = h.ok("project_validate", json!({ "path": "p" }));
    assert_eq!(v["valid"], true);
    let b = h.ok("backend_list", json!({}));
    assert!(
        b["backends"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == "verilator" && x["implemented"] == false)
    );
}

#[test]
fn qemu_project_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws]).unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };

    let backends = call("backend_list", json!({}));
    let have_qemu = backends["structuredContent"]["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true);
    if !have_qemu {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }

    let open = call("project_open", json!({ "path": "examples/math-qemu" }));
    assert_eq!(open["isError"], false, "{open}");
    assert_eq!(open["structuredContent"]["simulation_started"], true);

    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    assert_eq!(st["fidelity"], "f0");
    assert_eq!(st["ps_backend"], "qemu");

    let run = call("sim_run", json!({ "duration": "20ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let uart = call("uart_read", json!({ "port": 1 }));
    let text = uart["structuredContent"]["text"].as_str().unwrap();
    assert!(text.contains("RESULT: ALL PASS"), "{text}");

    // PL accesses are in the trace as bus events.
    let trace = call("trace_get", json!({ "from": 0, "max": 1000 }));
    let bus = trace["structuredContent"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["type"] == "bus")
        .count();
    assert_eq!(bus, 21);

    // Snapshot, restore, reset and replay work by re-executing the journal.
    let hash0 = call("status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    let r = call("snapshot_save", json!({ "name": "done" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call("sim_run", json!({ "duration": "2ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call("snapshot_restore", json!({ "name": "done" }));
    assert_eq!(r["isError"], false, "{r}");
    let hash1 = call("status_get", json!({}))["structuredContent"]["status"]["trace_hash"].clone();
    assert_eq!(hash1, hash0, "restore reproduces the snapshot digest");
    let rep = call("session_replay", json!({}));
    assert_eq!(rep["isError"], false, "{rep}");
    assert_eq!(rep["structuredContent"]["deterministic"], true, "{rep}");
    let r = call("sim_reset", json!({}));
    assert_eq!(r["isError"], false, "{r}");
    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    assert!(st["now_ps"].as_u64().unwrap() < 1_000_000, "{st}");
    let r = call("snapshot_restore", json!({ "name": "done" }));
    assert_eq!(r["isError"], true, "reset drops snapshots: {r}");

    // Capabilities the QEMU engine lacks are refused with the reason.
    for (tool, args) in [
        ("board_set_switch", json!({ "index": 0, "on": true })),
        ("cpu_regs", json!({})),
    ] {
        let r = call(tool, args);
        assert_eq!(r["isError"], true, "{tool}: {r}");
        let msg = r["structuredContent"]["error"]["message"].as_str().unwrap();
        assert!(msg.contains("qemu"), "{tool}: {msg}");
    }
}

/// P1 exit criterion: the prototype DMA regression under MCP control.
#[test]
fn dma_project_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws.clone()]).unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    let found = |n: &str| {
        backends
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == n && b["found"] == true)
    };
    if !found("qemu") || !found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    // The DPI library is a build product; build it if the tree lacks it.
    let lib = ws.join("target/dpi/release/libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = std::process::Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
            ])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    }

    let open = call("project_open", json!({ "path": "examples/dma-regression" }));
    assert_eq!(open["isError"], false, "{open}");
    assert_eq!(open["structuredContent"]["simulation_started"], true);
    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    assert_eq!(
        (st["ps_backend"].as_str(), st["pl_backend"].as_str()),
        (Some("qemu"), Some("xsim"))
    );

    // Run in slices, as an agent would, until the firmware reports.
    let mut text = String::new();
    for _ in 0..30 {
        let r = call("sim_run", json!({ "duration": "200us" }));
        assert_eq!(r["isError"], false, "{r}");
        let u = call("uart_read", json!({ "port": 1 }));
        text.push_str(u["structuredContent"]["text"].as_str().unwrap());
        if text.contains("COSIM_DONE") {
            break;
        }
    }
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
    assert_eq!(text.matches("PASS ").count(), 6, "{text}"); // the six cases

    // The trace shows both sides of the PS/PL boundary.
    let trace = call("trace_get", json!({ "from": 0, "max": 1000 }));
    let ports: std::collections::BTreeSet<String> = trace["structuredContent"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["type"] == "bus")
        .map(|r| r["port"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ports.contains("M_AXI_GP0") && ports.contains("S_AXI_HP0"),
        "{ports:?}"
    );
}

/// The virtual Zybo driven the way an agent would: shell over UART, switches
/// and buttons, LEDs, and DDR contents through `mem_read` / `mem_write`.
#[test]
fn board_demo_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws.clone()]).unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    let found = |n: &str| {
        backends
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == n && b["found"] == true)
    };
    if !found("qemu") || !found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    let lib = ws.join("target/dpi/release/libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = std::process::Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
                "--target-dir",
                "target/dpi",
            ])
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success());
    }

    let open = call("project_open", json!({ "path": "examples/board-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    let caps: Vec<&str> = st["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    for c in ["board_io", "uart", "memory_access"] {
        assert!(caps.contains(&c), "capability {c} missing: {caps:?}");
    }

    let text =
        |call: &mut dyn FnMut(&str, serde_json::Value) -> serde_json::Value, d: &str| -> String {
            let r = call("sim_run", json!({ "duration": d }));
            assert_eq!(r["isError"], false, "{r}");
            let u = call("uart_read", json!({ "port": 1 }));
            u["structuredContent"]["text"].as_str().unwrap().to_string()
        };

    // Boot, then drive the shell.
    let boot = text(&mut call, "6ms");
    assert!(boot.contains("ready"), "{boot}");

    let w = call("uart_write", json!({ "port": 1, "text": "led 10\r" }));
    assert_eq!(w["isError"], false, "{w}");
    let out = text(&mut call, "6ms");
    assert!(out.contains("LED=A"), "{out}");
    let leds = call("board_get_leds", json!({}))["structuredContent"]["leds"].clone();
    let on: Vec<bool> = leds
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["on"].as_bool().unwrap())
        .collect();
    assert_eq!(on, [false, true, false, true], "LED10 = 0b1010");
    assert_eq!(
        call("pin_get", json!({ "name": "led1" }))["structuredContent"]["level"],
        "1"
    );

    // A switch and a button reach the RTL.
    let r = call("board_set_switch", json!({ "id": "sw3", "on": true }));
    assert_eq!(r["isError"], false, "{r}");
    let _ = text(&mut call, "2ms");
    let r = call(
        "board_press_button",
        json!({ "id": "btn0", "hold": "200us" }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let _ = text(&mut call, "6ms");
    call("uart_write", json!({ "port": 1, "text": "status\r" }));
    let out = text(&mut call, "6ms");
    assert!(out.contains("SW=8"), "{out}");

    // Memory: read the firmware's image, write a pattern and read it back.
    let m = call("mem_read", json!({ "addr": 0x0100_0000u64, "len": 16 }));
    assert_eq!(m["isError"], false, "{m}");
    assert_eq!(m["structuredContent"]["hex"].as_str().unwrap().len(), 32);
    let w = call(
        "mem_write",
        json!({ "addr": 0x0200_0000u64, "hex": "deadbeef01020304" }),
    );
    assert_eq!(w["isError"], false, "{w}");
    let m = call("mem_read", json!({ "addr": 0x0200_0000u64, "len": 8 }));
    assert_eq!(m["structuredContent"]["hex"], "deadbeef01020304");

    // Registers are honestly unavailable, with the reason.
    let r = call("cpu_regs", json!({}));
    assert_eq!(r["isError"], true);
}

/// Audio and HDMI through MCP: capture what the codec played and what the
/// HDMI connector showed, and change what the board receives.
#[test]
fn audio_and_hdmi_demos_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws.clone()]).unwrap();
    let mut call_full = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call_full("backend_list", json!({}))["structuredContent"]["backends"].clone();
    let found = |n: &str| {
        backends
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == n && b["found"] == true)
    };
    if !found("qemu") || !found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    let lib = ws.join("target/dpi/release/libxsim_dpi.a");
    assert!(
        lib.is_file(),
        "build the DPI library first: cargo xtask dpi"
    );

    // ---- without a design that has media models the tools are refused ----
    let open = call_full("project_open", json!({ "path": "examples/math-qemu" }));
    assert_eq!(open["isError"], false, "{open}");
    let r = call_full("video_frame", json!({}));
    assert_eq!(r["isError"], true, "{r}");

    // ---- audio ----
    let open = call_full("project_open", json!({ "path": "examples/audio-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    let st = call_full("status_get", json!({}))["structuredContent"]["status"].clone();
    let caps: Vec<&str> = st["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    assert!(
        caps.contains(&"audio") && caps.contains(&"video"),
        "{caps:?}"
    );

    // No audio yet: a clear error, not an empty file.
    let r = call_full("audio_capture", json!({ "duration": "10ms" }));
    assert_eq!(r["isError"], true, "{r}");

    let r = call_full("media_source_set", json!({ "audio_in": "tone:440:0.5" }));
    assert_eq!(r["isError"], false, "{r}");
    // A file outside the allowed roots is refused.
    let r = call_full(
        "media_source_set",
        json!({ "audio_in": "C:/Windows/media/notify.wav" }),
    );
    assert_eq!(r["isError"], true, "{r}");

    let r = call_full("sim_run", json!({ "duration": "90ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let uart = call_full("uart_read", json!({ "port": 1 }))["structuredContent"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");

    // The newest 12 ms is the looped-back 440 Hz LINE IN.
    let a = call_full("audio_capture", json!({ "duration": "12ms" }));
    assert_eq!(a["isError"], false, "{a}");
    let sc = &a["structuredContent"];
    assert_eq!(sc["mime_type"], "audio/wav");
    assert_eq!(sc["sample_rate"], 48_000);
    let hz = sc["freq_hz"].as_f64().unwrap();
    assert!((hz - 440.0).abs() < 30.0, "{hz} Hz; {sc}");
    let peak = sc["peak"].as_i64().unwrap();
    assert!((15_000..=17_000).contains(&peak), "peak {peak}");
    let content = a["content"].as_array().unwrap();
    let audio = content
        .iter()
        .find(|c| c["type"] == "audio")
        .expect("an audio content block");
    assert_eq!(audio["mimeType"], "audio/wav");
    assert!(
        audio["data"].as_str().unwrap().starts_with("UklGR"),
        "RIFF in base64"
    );

    // ---- video ----
    let open = call_full("project_open", json!({ "path": "examples/hdmi-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    let r = call_full(
        "media_source_set",
        json!({ "video_in": "gradient:320x240" }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let r = call_full("video_frame", json!({}));
    assert_eq!(r["isError"], true, "no frame before the PL scans out: {r}");
    let r = call_full("sim_run", json!({ "duration": "40ms" }));
    assert_eq!(r["isError"], false, "{r}");
    let v = call_full("video_frame", json!({}));
    assert_eq!(v["isError"], false, "{v}");
    assert_eq!(v["structuredContent"]["width"], 320);
    assert_eq!(v["structuredContent"]["height"], 240);
    let img = v["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("an image content block");
    assert_eq!(img["mimeType"], "image/png");
    assert!(
        img["data"].as_str().unwrap().starts_with("iVBOR"),
        "PNG in base64"
    );
}

/// Debugging through MCP: stop at a symbol breakpoint, read registers and
/// symbols, set and clear breakpoints, continue.
#[test]
fn debugger_through_mcp() {
    let mut server = zynqsim_mcp::McpServer::new(vec![
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    ])
    .unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    if !backends
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
    {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }

    // Without [debug] the tools explain what to do.
    let open = call("project_open", json!({ "path": "examples/math-qemu" }));
    assert_eq!(open["isError"], false, "{open}");
    let r = call("cpu_regs", json!({}));
    assert_eq!(r["isError"], true);
    assert!(
        r["structuredContent"]["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("[debug]"),
        "{r}"
    );

    let open = call("project_open", json!({ "path": "examples/math-debug" }));
    assert_eq!(open["isError"], false, "{open}");
    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    let caps: Vec<&str> = st["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    assert!(caps.contains(&"cpu_state"), "{caps:?}");

    // Not stopped yet: a precise refusal.
    let r = call("cpu_regs", json!({}));
    assert_eq!(r["isError"], true, "{r}");

    let run = call("sim_run", json!({ "duration": "1ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let dbg = &run["structuredContent"]["debug"];
    assert_eq!(dbg["stopped"], true, "{run}");
    assert_eq!(dbg["pc"], 0x0010_058c);
    assert!(dbg["reason"].as_str().unwrap().contains("Xil_Out32"));
    // The call ended at the stop, long before 1 ms.
    assert!(run["structuredContent"]["now_ps"].as_u64().unwrap() < 200_000_000);

    let regs = call("cpu_regs", json!({ "core": 0 }));
    assert_eq!(regs["isError"], false, "{regs}");
    let sc = &regs["structuredContent"];
    assert_eq!(sc["regs"]["pc"], "0x0010058c");
    assert_eq!(sc["regs"]["r0"], "0xf8000008");
    assert_eq!(sc["regs"]["r1"], "0x0000df0d");
    assert_eq!(sc["mode"], "sys");
    assert_eq!(sc["pc_symbol"], "Xil_Out32");

    let sym = call("symbol_lookup", json!({ "query": "main" }));
    assert_eq!(sym["structuredContent"]["addr_hex"], "0x100614");
    let sym = call("symbol_lookup", json!({ "query": "0x10061c" }));
    assert_eq!(sym["structuredContent"]["name"], "main");
    assert_eq!(sym["structuredContent"]["offset"], 8);

    // Move the breakpoint to main's neighbour and add a watch on the PL.
    let list = call("breakpoint_list", json!({}));
    assert_eq!(
        list["structuredContent"]["breakpoints"][0]["symbol"],
        "Xil_Out32"
    );
    let r = call(
        "breakpoint_set",
        json!({ "addr": 0x10058c, "enable": false }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let list = call("breakpoint_list", json!({}));
    assert!(
        list["structuredContent"]["breakpoints"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Continue to the end.
    let run = call("sim_run", json!({ "duration": "2ms" }));
    assert_eq!(run["isError"], false, "{run}");
    assert_eq!(run["structuredContent"]["debug"]["stopped"], false);
    let uart = call("uart_read", json!({ "port": 1 }));
    assert!(
        uart["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .contains("RESULT: ALL PASS"),
        "{uart}"
    );
    // Disassembly works on paused memory and names the symbols.
    let d = call("cpu_disasm", json!({ "addr": 0x100614, "count": 6 }));
    assert_eq!(d["isError"], false, "{d}");
    let ins = d["structuredContent"]["instructions"].as_array().unwrap();
    assert_eq!(ins.len(), 6);
    assert_eq!(ins[0]["addr"], 0x100614);
    assert_eq!(ins[0]["symbol"], "main");
    assert_eq!(ins[1]["symbol"], "main+0x4");
    assert!(
        ins.iter()
            .all(|i| !i["text"].as_str().unwrap().starts_with('.')),
        "{d}"
    );
    assert_eq!(d["structuredContent"]["thumb"], false);
    // Bad arguments are refused.
    let r = call("cpu_disasm", json!({ "addr": 0x100614, "count": 0 }));
    assert_eq!(r["isError"], true);
}

/// Pmod OLED through MCP: the device declared in the project shows the text
/// the firmware drew; attaching works at run time and is refused when it
/// cannot.
#[test]
fn pmod_oled_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws.clone()]).unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    let found = |n: &str| {
        backends
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == n && b["found"] == true)
    };
    if !found("qemu") || !found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    assert!(ws.join("target/dpi/release/libxsim_dpi.a").is_file());

    // A design without Pmod pins refuses the tools.
    let open = call("project_open", json!({ "path": "examples/math-qemu" }));
    assert_eq!(open["isError"], false, "{open}");
    let r = call("pmod_attach", json!({ "port": "JA", "module": "oled" }));
    assert_eq!(r["isError"], true, "{r}");

    let open = call("project_open", json!({ "path": "examples/oled-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    let st = call("status_get", json!({}))["structuredContent"]["status"].clone();
    assert!(
        st["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "pmod"),
        "{st}"
    );
    let before = call("pmod_frame", json!({ "port": "JA" }));
    assert_eq!(before["isError"], false, "{before}");
    assert!(
        before["structuredContent"]["state"]
            .as_str()
            .unwrap()
            .contains("off")
    );

    let run = call("sim_run", json!({ "duration": "4ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let f = call("pmod_frame", json!({ "port": "JA" }));
    assert_eq!(f["isError"], false, "{f}");
    let sc = &f["structuredContent"];
    assert_eq!(
        (sc["width"].as_u64(), sc["height"].as_u64()),
        (Some(128), Some(32))
    );
    assert!(sc["state"].as_str().unwrap().contains("on"), "{sc}");
    let img = f["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("an image block");
    assert!(img["data"].as_str().unwrap().starts_with("iVBOR"));

    // The pins are visible too.
    let pin = call("pin_get", json!({ "name": "JA1" }));
    assert_eq!(pin["isError"], false, "{pin}");
    assert!(["0", "1"].contains(&pin["structuredContent"]["level"].as_str().unwrap()));

    // Attach: a second device on JA is refused, JB takes one, JF is not a PL port.
    let r = call("pmod_attach", json!({ "port": "JA", "module": "oled" }));
    assert_eq!(r["isError"], true, "{r}");
    let r = call("pmod_attach", json!({ "port": "JB", "module": "oled" }));
    assert_eq!(r["isError"], false, "{r}");
    let r = call("pmod_attach", json!({ "port": "JF", "module": "oled" }));
    assert_eq!(r["isError"], true, "{r}");
    let r = call("pmod_attach", json!({ "port": "JC", "module": "toaster" }));
    assert_eq!(r["isError"], true, "{r}");
}

/// Pmod sensors through MCP: `pmod_set` changes what a sensor measures while
/// the firmware keeps sampling; `pmod_state` reads the model back. Errors say
/// what is accepted.
#[test]
fn pmod_sensors_through_mcp() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws.clone()]).unwrap();
    let mut call = |name: &str, args: serde_json::Value| -> serde_json::Value {
        server
            .handle(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": name, "arguments": args } }),
                &mut zynqsim_mcp::NoControl,
            )
            .unwrap()["result"]
            .clone()
    };
    let backends = call("backend_list", json!({}))["structuredContent"]["backends"].clone();
    let found = |n: &str| {
        backends
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == n && b["found"] == true)
    };
    if !found("qemu") || !found("xsim") {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    assert!(ws.join("target/dpi/release/libxsim_dpi.a").is_file());

    // Without a design that reports Pmod pins there is nothing to set.
    let open = call("project_open", json!({ "path": "examples/math-qemu" }));
    assert_eq!(open["isError"], false, "{open}");
    let r = call(
        "pmod_set",
        json!({ "port": "JA", "name": "temperature", "value": 1.0 }),
    );
    assert_eq!(r["isError"], true, "{r}");
    let r = call("pmod_state", json!({ "port": "JA" }));
    assert_eq!(r["isError"], true, "{r}");

    // The temperature sensor on I2C.
    let open = call("project_open", json!({ "path": "examples/tmp2-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    let run = call("sim_run", json!({ "duration": "6ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let uart = call("uart_read", json!({ "port": 1 }));
    let text = uart["structuredContent"]["text"].as_str().unwrap();
    assert!(text.contains("T16 raw=0x0BC0 mC=23500"), "{text}");

    let st = call("pmod_state", json!({ "port": "JA" }));
    assert_eq!(st["isError"], false, "{st}");
    let sc = &st["structuredContent"];
    assert_eq!(sc["module"], "tmp2");
    assert_eq!(sc["values"][0]["name"], "temperature");
    assert_eq!(sc["values"][0]["value"], 23.5);
    assert_eq!(sc["values"][0]["unit"], "degC");
    assert!(
        sc["registers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "ID" && r["value"] == 0xCB),
        "{sc}"
    );

    let set = call(
        "pmod_set",
        json!({ "port": "JA", "name": "temperature", "value": 31.25 }),
    );
    assert_eq!(set["isError"], false, "{set}");
    assert_eq!(set["structuredContent"]["values"][0]["value"], 31.25);
    let run = call("sim_run", json!({ "duration": "4ms" }));
    assert_eq!(run["isError"], false, "{run}");
    let uart = call("uart_read", json!({ "port": 1 }));
    let text = uart["structuredContent"]["text"].as_str().unwrap();
    assert!(
        text.contains("T16 raw=0x0FA0 mC=31250"),
        "31.25 degC read back by the firmware: {text}"
    );

    // Out-of-range values are clamped, unknown quantities and empty ports are
    // errors that say what is accepted.
    let r = call(
        "pmod_set",
        json!({ "port": "JA", "name": "temperature", "value": 900.0 }),
    );
    assert_eq!(r["isError"], false, "{r}");
    assert_eq!(r["structuredContent"]["values"][0]["value"], 150.0);
    let r = call(
        "pmod_set",
        json!({ "port": "JA", "name": "humidity", "value": 1.0 }),
    );
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("temperature"), "{r}");
    let r = call(
        "pmod_set",
        json!({ "port": "JB", "name": "temperature", "value": 1.0 }),
    );
    assert_eq!(r["isError"], true, "{r}");
    let r = call(
        "pmod_set",
        json!({ "port": "JA", "name": "temperature", "value": "hot" }),
    );
    assert_eq!(r["isError"], true, "{r}");

    // The accelerometer on SPI mode 3.
    let open = call("project_open", json!({ "path": "examples/acl-demo" }));
    assert_eq!(open["isError"], false, "{open}");
    call("sim_run", json!({ "duration": "2ms" }));
    let set = call(
        "pmod_set",
        json!({ "port": "JA", "name": "accel_y", "value": 0.75 }),
    );
    assert_eq!(set["isError"], false, "{set}");
    call("sim_run", json!({ "duration": "2ms" }));
    let uart = call("uart_read", json!({ "port": 1 }));
    let text = uart["structuredContent"]["text"].as_str().unwrap();
    assert!(text.contains("ACC x=500 y=-250 z=1000 mg"), "{text}");
    assert!(text.contains("ACC x=500 y=750 z=1000 mg"), "{text}");
    let st = call("pmod_state", json!({ "port": "JA" }));
    let names: Vec<&str> = st["structuredContent"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["accel_x", "accel_y", "accel_z"]);
}
