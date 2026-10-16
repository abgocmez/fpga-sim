//! Boards other than the Zybo through MCP (ADR-022): the tools validate `index` against the
//! board in use, `pin_get` and `status_get` speak the board's names, and a project can bring
//! its own board file. Demo engine only: no external tool is needed.

mod common;

use base64::Engine as _;
use common::Harness;
use serde_json::{Value, json};

const BOARD: &str = include_str!("../../../examples/custom-board/my-board.toml");
const SVG: &str = include_str!("../../../examples/custom-board/my-board.svg");

fn zedboard() -> Harness {
    let mut h = Harness::new("boards");
    std::fs::create_dir_all(h.root.join("z")).unwrap();
    std::fs::write(
        h.root.join("z/project.toml"),
        "schema_version = 1\n[board]\nname = \"zedboard\"\n",
    )
    .unwrap();
    let open = h.ok("project_open", json!({ "path": "z" }));
    assert_eq!(open["simulation_started"], true, "{open}");
    h
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn status_names_the_board_and_its_controls() {
    let mut h = zedboard();
    let st = h.ok("status_get", json!({}));
    assert_eq!(st["status"]["board"], "zedboard");
    let b = &st["board_info"];
    assert_eq!(b["display_name"], "ZedBoard");
    assert_eq!(
        ids(&b["switches"]),
        (0..8).map(|i| format!("sw{i}")).collect::<Vec<_>>()
    );
    assert_eq!(ids(&b["buttons"]).len(), 7);
    assert_eq!(ids(&b["leds"]).len(), 8);
    assert_eq!(ids(&b["pmods"]), ["JA", "JB", "JC", "JD"]);

    // The board resource is the same description.
    let r = h.rpc("resources/read", json!({ "uri": "zynqsim://board" }));
    let text = r["result"]["contents"][0]["text"].as_str().unwrap();
    let def: Value = serde_json::from_str(text).unwrap();
    assert_eq!(def["name"], "zedboard");
    assert_eq!(def["switches"].as_array().unwrap().len(), 8);
    assert!(
        def.get("svg_source").is_none(),
        "artwork text is not exported"
    );
}

#[test]
fn switch_and_button_indices_are_validated_against_the_board() {
    let mut h = zedboard();
    // The last switch and button of the ZedBoard are beyond the Zybo's ranges.
    let r = h.ok("board_set_switch", json!({ "index": 7, "on": true }));
    assert_eq!(r["switch"], "sw7");
    let r = h.ok("board_set_switch", json!({ "id": "sw6", "on": true }));
    assert_eq!(r["switch"], "sw6");
    h.err(
        "board_set_switch",
        json!({ "index": 8, "on": true }),
        "switch index 8 out of range 0..8",
    );
    let code = h.err(
        "board_set_switch",
        json!({ "id": "sw8", "on": true }),
        "no switch with id `sw8`",
    );
    assert_eq!(code, "not_found");

    let r = h.ok(
        "board_press_button",
        json!({ "index": 6, "action": "down" }),
    );
    assert_eq!(r["button"], "btn6");
    h.err(
        "board_press_button",
        json!({ "index": 7, "action": "down" }),
        "button index 7 out of range 0..7",
    );
    h.err(
        "board_press_button",
        json!({ "id": "btn7" }),
        "no button with id `btn7`",
    );
    // Giving both, or neither, is refused whatever the board.
    h.err(
        "board_press_button",
        json!({ "index": 1, "id": "btn1" }),
        "exactly one of `index` or `id`",
    );
}

#[test]
fn pin_get_and_leds_follow_the_board() {
    let mut h = zedboard();
    h.ok("board_set_switch", json!({ "index": 6, "on": true }));
    assert_eq!(h.ok("pin_get", json!({ "name": "sw6" }))["level"], "1");
    // The demo engine mirrors SW6 on LED6; LED7 is its heartbeat.
    assert_eq!(h.ok("pin_get", json!({ "name": "led6" }))["level"], "1");
    assert_eq!(h.ok("pin_get", json!({ "name": "btn6" }))["level"], "0");
    assert_eq!(h.ok("pin_get", json!({ "name": "JD10" }))["level"], "Z");

    let leds = h.ok("board_get_leds", json!({}));
    assert_eq!(leds["leds"].as_array().unwrap().len(), 8);
    assert_eq!(leds["rgb_leds"].as_array().unwrap().len(), 0);
    assert_eq!(leds["leds"][6]["id"], "led6");
    assert_eq!(leds["leds"][6]["on"], true);

    // A name from another board is unknown, and the hint lists this board's.
    let r = h.call("pin_get", json!({ "name": "led5_g" }));
    assert_eq!(r["isError"], true);
    let e = &r["structuredContent"]["error"];
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("no pin or control named `led5_g`"),
        "{e}"
    );
    assert!(
        e["hint"]
            .as_str()
            .unwrap()
            .contains("sw0..sw7, btn0..btn6, led0..led7, JA1..JD10"),
        "{e}"
    );
}

#[test]
fn a_project_can_bring_its_own_board_file() {
    let mut h = Harness::new("custom");
    let d = h.root.join("cb");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("my-board.toml"), BOARD).unwrap();
    std::fs::write(d.join("my-board.svg"), SVG).unwrap();
    std::fs::write(
        d.join("project.toml"),
        "schema_version = 1\n[board]\nfile = \"my-board.toml\"\n",
    )
    .unwrap();
    let open = h.ok("project_open", json!({ "path": "cb" }));
    assert_eq!(open["simulation_started"], true, "{open}");

    let st = h.ok("status_get", json!({}));
    assert_eq!(st["status"]["board"], "lab-board");
    assert_eq!(ids(&st["board_info"]["switches"]), ["sw0", "sw1", "sw2"]);
    assert_eq!(ids(&st["board_info"]["leds"]).len(), 6);
    assert_eq!(ids(&st["board_info"]["rgb_leds"]), ["rgb0"]);

    h.ok("board_set_switch", json!({ "index": 2, "on": true }));
    h.err(
        "board_set_switch",
        json!({ "index": 3, "on": true }),
        "switch index 3 out of range 0..3",
    );
    h.err(
        "board_press_button",
        json!({ "index": 2 }),
        "button index 2 out of range 0..2",
    );
    // The screenshot is the custom layout (600x360) drawn with its artwork.
    let shot = h.call("board_screenshot", json!({}));
    assert_eq!(shot["isError"], false, "{shot}");
    let img = shot["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("an image part");
    let png = base64::engine::general_purpose::STANDARD
        .decode(img["data"].as_str().unwrap())
        .unwrap();
    let (w, hgt, _) = zynqsim_mcp_decode(&png);
    assert_eq!((w, hgt), (600, 360));
}

/// `(width, height, rgb)` of a PNG, through the renderer's own decoder.
fn zynqsim_mcp_decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
    sim_api::board::decode_png_rgb(png).unwrap()
}

#[test]
fn a_board_file_with_mistakes_is_refused_with_every_one_of_them() {
    let mut h = Harness::new("badboard");
    let d = h.root.join("bad");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("b.toml"),
        BOARD
            .replace("kind = \"soc\"", "kind = \"cpu\"")
            .replace("x = 90\ny = 290", "x = 9000\ny = 290")
            .replace("id = \"sw1\"", "id = \"sw0\""),
    )
    .unwrap();
    std::fs::write(
        d.join("project.toml"),
        "schema_version = 1\n[board]\nfile = \"b.toml\"\n",
    )
    .unwrap();
    let r = h.call("project_open", json!({ "path": "bad" }));
    let text = r.to_string();
    for want in [
        "board.file",
        "3 problem(s)",
        "unknown kind `cpu`",
        "outside the 600x360 layout",
        "duplicate id `sw0`",
    ] {
        assert!(text.contains(want), "missing `{want}` in {text}");
    }
    // The old simulation (none here) is not replaced by a broken one.
    assert_eq!(r["structuredContent"]["simulation_started"], false, "{r}");
}
