//! `waveform_get` through MCP on real simulations: the built-in demo engine
//! (no external tools) and the QEMU + native-PL example (Xilinx QEMU).

mod common;

use base64::Engine as _;
use common::Harness;
use serde_json::{Value, json};

fn changes(sig: &Value) -> Vec<(u64, u64)> {
    sig["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c[0].as_u64().unwrap(), c[1].as_u64().unwrap()))
        .collect()
}

fn signal<'a>(w: &'a Value, name: &str) -> &'a Value {
    w["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no signal {name} in {w}"))
}

#[test]
fn waveform_get_on_the_demo_engine() {
    let mut h = Harness::new("wave");
    h.ok("project_open", json!({ "path": "p" }));
    // Drive the board at known times: switch 0 on at 1 ms, off at 3 ms.
    h.ok("sim_run", json!({ "duration": "1ms" }));
    h.ok("board_set_switch", json!({ "index": 0, "on": true }));
    h.ok("sim_run", json!({ "duration": "2ms" }));
    h.ok("board_set_switch", json!({ "index": 0, "on": false }));
    h.ok("sim_run", json!({ "duration": "1ms" }));

    // No signals: the catalogue.
    let cat = h.ok("waveform_get", json!({}));
    let names: Vec<&str> = cat["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    for want in ["board.sw0", "board.led0", "board.led3", "uart1.tx"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let now = cat["now_ps"].as_u64().unwrap();
    assert!((3_000_000_000..=4_000_000_000).contains(&now), "{now}");

    // The switch: driven low at reset, high at 1 ms, low at 3 ms.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.sw0", "uart1.tx", "nope"], "from": "0", "to": "4ms" }),
    );
    assert_eq!(w["truncated"], false);
    assert_eq!(w["to_ps"], 4_000_000_000u64);
    let sw = signal(&w, "board.sw0");
    assert_eq!(sw["width"], 1);
    assert_eq!(
        changes(sw),
        [(0, 0), (1_000_000_000, 1), (3_000_000_000, 0)],
        "{sw}"
    );
    assert_eq!(signal(&w, "nope")["found"], false);
    assert_eq!(signal(&w, "uart1.tx")["found"], true);

    // A window inside the high pulse: no changes, the initial value is 1.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.sw0"], "from": "1500us", "to": "2500us" }),
    );
    let sw = signal(&w, "board.sw0");
    assert!(changes(sw).is_empty());
    assert_eq!(sw["initial"], 1);

    // The LED follows the switch in the demo firmware; wildcards expand in
    // name order and the request order of exact names is kept.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.sw0", "board.led*"] }),
    );
    let order: Vec<&str> = w["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(order[0], "board.sw0");
    assert!(order.contains(&"board.led0") && order.contains(&"board.led3"));
    let led0 = changes(signal(&w, "board.led0"));
    assert!(
        led0.iter().any(|c| c.0 >= 1_000_000_000 && c.1 != 0),
        "{led0:?}"
    );

    // Truncation is explicit and the covered time says where it stops.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.led*"], "max_changes": 1 }),
    );
    // Every LED has at least its reset sample; the one that switched has more.
    let led0 = signal(&w, "board.led0");
    assert_eq!(led0["truncated"], true, "{led0}");
    assert_eq!(led0["changes"].as_array().unwrap().len(), 1);
    assert!(led0["in_window"].as_u64().unwrap() > 1);
    assert_eq!(w["truncated"], true);
    assert!(led0["covered_to_ps"].as_u64().unwrap() < 4_000_000_000u64);

    // Bad input is refused with a reason.
    h.err(
        "waveform_get",
        json!({ "signals": ["board.sw0"], "from": "3ms", "to": "1ms" }),
        "before it starts",
    );
    h.err("waveform_get", json!({ "signals": [1] }), "signals");
    h.err(
        "waveform_get",
        json!({ "signals": ["board.sw0"], "render": "gif" }),
        "one of",
    );
}

#[test]
fn waveform_get_renders_svg_and_png() {
    let mut h = Harness::new("wavepic");
    h.ok("project_open", json!({ "path": "p" }));
    h.ok("sim_run", json!({ "duration": "1ms" }));
    h.ok("board_set_switch", json!({ "index": 1, "on": true }));
    h.ok("sim_run", json!({ "duration": "1ms" }));

    // Inline SVG.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.sw1", "board.led1"], "render": "svg", "width": 600 }),
    );
    let svg = w["svg"].as_str().unwrap();
    assert!(
        svg.starts_with("<svg") && svg.contains("board.sw1 [1]"),
        "{svg}"
    );
    assert!(svg.contains("board.led1 [8]"));

    // SVG to a file inside the root: not repeated inline.
    let w = h.ok(
        "waveform_get",
        json!({ "signals": ["board.sw1"], "render": "svg", "path": "wave.svg" }),
    );
    assert!(w.get("svg").is_none());
    let text = std::fs::read_to_string(h.root.join("wave.svg")).unwrap();
    assert_eq!(text.len() as u64, w["svg_bytes"].as_u64().unwrap());
    h.err(
        "waveform_get",
        json!({ "signals": ["board.sw1"], "render": "svg", "path": "../escape.svg" }),
        "outside",
    );

    // PNG: an image content item with the picture's size.
    let r = h.call(
        "waveform_get",
        json!({ "signals": ["board.sw1", "board.led1"], "render": "png", "width": 640,
                "path": "wave.png" }),
    );
    assert_eq!(r["isError"], false, "{r}");
    let img = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("an image content item");
    assert_eq!(img["mimeType"], "image/png");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(img["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let s = &r["structuredContent"]["image"];
    assert_eq!(s["width"], 640);
    assert_eq!(s["height"], 22 + 2 * 28);
    // IHDR carries the same size.
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 640);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 78);
    assert_eq!(std::fs::read(h.root.join("wave.png")).unwrap(), bytes);
}

/// Bus accesses of a real PS firmware talking to a native PL model are
/// derived signals; this needs the Xilinx QEMU and prints `SKIPPED:` without.
#[test]
fn waveform_get_shows_the_bus_of_a_qemu_project() {
    let ws = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut server = zynqsim_mcp::McpServer::new(vec![ws]).unwrap();
    let mut call = |name: &str, args: Value| -> Value {
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
    let run = call("sim_run", json!({ "duration": "20ms" }));
    assert_eq!(run["isError"], false, "{run}");

    let cat = call("waveform_get", json!({}))["structuredContent"].clone();
    let sig = |n: &str| {
        cat["signals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .cloned()
    };
    let addr = sig("bus.M_AXI_GP0.addr").expect("derived address signal");
    assert_eq!(addr["derived"], true);
    assert_eq!(addr["changes"], 21, "the firmware makes 21 PL accesses");
    assert_eq!(sig("uart1.tx").expect("uart")["width"], 8);

    let w = call(
        "waveform_get",
        json!({ "signals": ["bus.M_AXI_GP0.*"], "render": "png" }),
    );
    assert_eq!(w["isError"], false, "{w}");
    let w = &w["structuredContent"];
    assert_eq!(w["truncated"], false);
    let a = changes(signal(w, "bus.M_AXI_GP0.addr"));
    assert_eq!(a.len(), 21);
    assert!(
        a.iter().all(|c| (0x43C0_0000..0x43C1_0000).contains(&c.1)),
        "addresses are in the IP's window: {a:x?}"
    );
    assert!(a.windows(2).all(|p| p[0].0 <= p[1].0), "time ordered");
    let wr = changes(signal(w, "bus.M_AXI_GP0.write"));
    assert!(wr.iter().any(|c| c.1 == 1) && wr.iter().any(|c| c.1 == 0));
    // The result the firmware printed is also one of the data values.
    assert_eq!(signal(w, "bus.M_AXI_GP0.data")["width"], 32);
}
