//! Boards through the `zynqsim` binary (ADR-022): `run --json` reports the board's own
//! switches, buttons and LEDs, `--at` targets and range errors come from the board,
//! `inspect --board` summarises and validates board files, and a project can name a file.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    assert_eq!(o.status.code(), Some(0), "{}", stderr(o));
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

fn example(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(rel)
        .display()
        .to_string()
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-boards-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn run_json_reports_the_boards_own_controls() {
    // The default board keeps its four of each.
    let v = json(&zynqsim(&[
        "--json",
        "run",
        "--for",
        "5ms",
        "--at",
        "1ms=sw3:on",
    ]));
    assert_eq!(v["board"], "zybo-z7-20");
    assert_eq!(v["board_info"]["switches"], 4);
    assert_eq!(v["switches"].as_array().unwrap().len(), 4);
    assert_eq!(v["buttons"].as_array().unwrap().len(), 4);
    assert_eq!(v["leds"].as_array().unwrap().len(), 4);
    assert_eq!(
        v["switches"][3],
        serde_json::json!({ "id": "sw3", "on": true })
    );

    // The ZedBoard has eight of the switches and LEDs and seven buttons.
    let v = json(&zynqsim(&[
        "--json",
        "run",
        "--board",
        "zedboard",
        "--for",
        "5ms",
        "--at",
        "1ms=sw6:on",
        "--at",
        "2ms=btn6:down",
    ]));
    assert_eq!(v["board"], "zedboard");
    assert_eq!(
        v["board_info"],
        serde_json::json!({
            "name": "zedboard", "display_name": "ZedBoard", "switches": 8, "buttons": 7,
            "leds": 8, "rgb_leds": 0, "pmods": 4
        })
    );
    assert_eq!(v["switches"].as_array().unwrap().len(), 8);
    assert_eq!(v["buttons"].as_array().unwrap().len(), 7);
    assert_eq!(v["leds"].as_array().unwrap().len(), 8);
    assert_eq!(v["switches"][6]["on"], true);
    assert_eq!(
        v["buttons"][6],
        serde_json::json!({ "id": "btn6", "pressed": true })
    );
    assert_eq!(
        v["leds"][6]["duty"], 255,
        "LED6 follows SW6 on the demo engine"
    );

    // PYNQ-Z2: two switches only. sw2 is not a control there, and the error says so.
    let o = zynqsim(&[
        "run",
        "--board",
        "pynq-z2",
        "--for",
        "1ms",
        "--at",
        "0ms=sw2:on",
    ]);
    assert_eq!(o.status.code(), Some(2));
    let e = stderr(&o);
    assert!(
        e.contains("unknown target `sw2`; use one of sw0, sw1, btn0, btn1, btn2, btn3"),
        "{e}"
    );
}

#[test]
fn a_project_with_a_board_file_runs_and_inspects() {
    let p = example("custom-board/project.toml");
    let v = json(&zynqsim(&[
        "--json",
        "run",
        "--project",
        &p,
        "--for",
        "10ms",
        "--at",
        "2ms=sw2:on",
    ]));
    assert_eq!(v["board"], "lab-board");
    assert_eq!(v["board_info"]["leds"], 6);
    assert_eq!(v["leds"][2]["duty"], 255);
    assert_eq!(v["rgb"].as_array().unwrap().len(), 1);

    let v = json(&zynqsim(&["--json", "inspect", "--project", &p]));
    assert_eq!(v["board"]["name"], "lab-board");
    assert_eq!(v["board"]["svg"], "my-board.svg");
    assert_eq!(v["board"]["resources"]["luts"], 53200);
    assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
    assert_eq!(
        v["builtin_boards"],
        serde_json::json!(["zybo-z7-20", "zybo-z7-10", "zedboard", "pynq-z2"])
    );
}

#[test]
fn inspect_board_summarises_built_in_boards_and_board_files() {
    let v = json(&zynqsim(&["--json", "inspect", "--board", "zybo-z7-10"]));
    assert_eq!(v["board"]["part"], "xc7z010clg400-1");
    assert_eq!(v["board"]["resources"]["luts"], 17600);
    let v = json(&zynqsim(&["--json", "inspect", "--board", "pynq-z2"]));
    assert_eq!(
        (v["board"]["switches"].clone(), v["board"]["pmods"].clone()),
        (serde_json::json!(2), serde_json::json!(2))
    );
    let o = zynqsim(&["inspect", "--board", &example("custom-board/my-board.toml")]);
    assert_eq!(o.status.code(), Some(0));
    let t = stdout(&o);
    assert!(
        t.contains("Lab board (example)") && t.contains("3 switches, 2 buttons, 6 LEDs"),
        "{t}"
    );

    // Neither a built-in board nor a file.
    let o = zynqsim(&["inspect", "--board", "nexys"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("neither a built-in board (zybo-z7-20, zybo-z7-10, zedboard, pynq-z2)"),
        "{}",
        stderr(&o)
    );

    // A board file with mistakes: exit 1, every mistake listed.
    let d = temp("bad");
    let text = std::fs::read_to_string(example("custom-board/my-board.toml")).unwrap();
    std::fs::write(d.join("svg.svg"), "<svg viewBox=\"0 0 1 1\"/>").unwrap();
    let bad = d.join("bad.toml");
    std::fs::write(
        &bad,
        text.replace("kind = \"soc\"", "kind = \"cpu\"")
            .replace("pin = \"A2\"", "pin = \"A1\"")
            .replace("my-board.svg", "svg.svg"),
    )
    .unwrap();
    let o = zynqsim(&["inspect", "--board", &bad.display().to_string()]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let e = stderr(&o);
    assert!(e.contains("2 problem(s)"), "{e}");
    assert!(e.contains("unknown kind `cpu`"), "{e}");
    assert!(
        e.contains("pin `A1` is used by both switch `sw0` and switch `sw1`"),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_project_whose_board_file_is_wrong_is_refused_with_the_field() {
    let d = temp("proj");
    std::fs::write(
        d.join("project.toml"),
        "schema_version = 1\n[board]\nfile = \"missing.toml\"\n",
    )
    .unwrap();
    let o = zynqsim(&[
        "run",
        "--project",
        &d.join("project.toml").display().to_string(),
    ]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let e = stderr(&o);
    assert!(e.contains("board.file: cannot read the board file"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}
