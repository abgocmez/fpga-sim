//! Custom and built-in boards through `[board]` in `project.toml` (ADR-022): resolution and
//! its errors, board-sized state and input validation on the demo engine, recordings that
//! carry a custom board, and (needs QEMU and XSim, `SKIPPED:` otherwise) the ZedBoard's
//! 8 LEDs, 8 switches and 7 buttons through a real RTL testbench.

use sim_api::{
    Capability, Command, Engine, Input, Project, Query, Recording, Response, Severity, SimConfig,
    Simulator,
};
use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn project(board: &str) -> Project {
    Project::from_toml(&format!("schema_version = 1\n[board]\n{board}\n")).unwrap()
}

fn errors(p: &Project, base: &Path) -> Vec<(String, String)> {
    p.validate(base)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.field, d.message))
        .collect()
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-boards-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn every_builtin_board_name_validates_and_unknown_names_list_the_choices() {
    for name in ["zybo-z7-20", "zybo-z7-10", "zedboard", "pynq-z2"] {
        let p = project(&format!("name = \"{name}\""));
        assert_eq!(errors(&p, Path::new(".")), vec![], "{name}");
        assert_eq!(p.board_def(Path::new(".")).unwrap().name, name);
    }
    let e = errors(&project("name = \"nexys\""), Path::new("."));
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].0, "board.name");
    assert!(
        e[0].1
            .contains("unknown board `nexys`; built-in: zybo-z7-20, zybo-z7-10, zedboard, pynq-z2"),
        "{}",
        e[0].1
    );
    assert!(e[0].1.contains("file = "), "{}", e[0].1);

    // Neither a name nor a file.
    let e = errors(&project("ddr_mb = 512"), Path::new("."));
    assert_eq!(e[0].0, "board.name");
    assert!(e[0].1.contains("needs `name`"), "{}", e[0].1);
}

#[test]
fn a_board_file_resolves_next_to_the_project_and_its_mistakes_are_precise() {
    let d = scratch("file");
    let good = std::fs::read_to_string(examples().join("custom-board/my-board.toml")).unwrap();
    std::fs::write(d.join("a.toml"), &good).unwrap();
    std::fs::write(
        d.join("my-board.svg"),
        std::fs::read(examples().join("custom-board/my-board.svg")).unwrap(),
    )
    .unwrap();

    // The file is found relative to the project; the name is optional.
    let p = project("file = \"a.toml\"");
    assert_eq!(errors(&p, &d), vec![]);
    let def = p.board_def(&d).unwrap();
    assert_eq!(
        (def.name.as_str(), def.switches.len(), def.leds.len()),
        ("lab-board", 3, 6)
    );
    assert!(def.svg_source.is_some());
    // From another directory it is not found, naming the path it tried.
    let e = errors(&p, Path::new("somewhere-else"));
    assert_eq!(e[0].0, "board.file");
    assert!(e[0].1.contains("cannot read the board file"), "{}", e[0].1);

    // A name that disagrees with the file's own.
    let e = errors(&project("name = \"zedboard\"\nfile = \"a.toml\""), &d);
    assert_eq!(e[0].0, "board.name");
    assert!(
        e[0].1.contains("does not match the name `lab-board`"),
        "{}",
        e[0].1
    );
    // ... and one that agrees is fine.
    assert_eq!(
        errors(&project("name = \"lab-board\"\nfile = \"a.toml\""), &d),
        vec![]
    );

    // A file with several mistakes reports every one.
    std::fs::write(
        d.join("bad.toml"),
        good.replace("kind = \"soc\"", "kind = \"cpu\"")
            .replace("id = \"sw1\"", "id = \"sw0\""),
    )
    .unwrap();
    let e = errors(&project("file = \"bad.toml\""), &d);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].0, "board.file");
    assert!(e[0].1.contains("2 problem(s)"), "{}", e[0].1);
    assert!(e[0].1.contains("duplicate id `sw0`"), "{}", e[0].1);
    assert!(e[0].1.contains("unknown kind `cpu`"), "{}", e[0].1);

    // The same text is what build_engine refuses with.
    let err = project("file = \"bad.toml\"")
        .build_engine(&d)
        .err()
        .unwrap();
    assert!(err.message.contains("board.file:"), "{}", err.message);

    // The project round-trips through TOML without inventing a `name`.
    let text = p.to_toml().unwrap();
    assert!(
        text.contains("file = \"a.toml\"") && !text.contains("name ="),
        "{text}"
    );
    assert_eq!(Project::from_toml(&text).unwrap(), p);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn devices_must_fit_the_boards_ports() {
    let base = "schema_version = 1\n[board]\nname = \"pynq-z2\"\n[devices]\n";
    // PYNQ-Z2 has two Pmod ports: pmod_ja is the first, pmod_jb the second.
    let ok = Project::from_toml(&format!("{base}pmod_jb = \"oled\"\n")).unwrap();
    assert_eq!(errors(&ok, Path::new(".")), vec![]);
    let bad = Project::from_toml(&format!("{base}pmod_jc = \"oled\"\ncam = \"pcam\"\n")).unwrap();
    let e = errors(&bad, Path::new("."));
    let msg = |f: &str| e.iter().find(|x| x.0 == f).map(|x| x.1.clone());
    assert!(
        msg("devices.pmod_jc").is_some_and(|m| m.contains("has 2 Pmod port(s) (PMODA, PMODB)")),
        "{e:?}"
    );
    assert!(
        msg("devices.cam").is_some_and(|m| m.contains("has no camera connector")),
        "{e:?}"
    );
    // The Zybo keeps all five and the camera.
    let z = Project::from_toml(
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[devices]\npmod_je = \"oled\"\ncam = \"pcam\"\n",
    )
    .unwrap();
    assert_eq!(errors(&z, Path::new(".")), vec![]);
}

fn engine_for(name: &str) -> Box<dyn Engine> {
    project(&format!("name = \"{name}\""))
        .build_engine(Path::new("."))
        .unwrap_or_else(|e| panic!("{e}"))
}

fn input(e: &mut dyn Engine, i: Input) -> Result<(), String> {
    e.command(Command::Input(i))
        .map(|_| ())
        .map_err(|x| x.message)
}

#[test]
fn the_demo_engine_sizes_its_state_and_validates_inputs_from_the_board() {
    // (name, switches, buttons, LEDs)
    for (name, sw, btn, led) in [
        ("zybo-z7-20", 4, 4, 4),
        ("zedboard", 8, 7, 8),
        ("pynq-z2", 2, 4, 4),
    ] {
        let mut e = engine_for(name);
        assert_eq!(e.def().name, name);
        let st = e.board().clone();
        assert_eq!(
            (st.switches.len(), st.buttons.len(), st.leds.len()),
            (sw, btn, led),
            "{name}"
        );
        assert_eq!(e.status(false).board, name);

        // The last component is addressable, one past it is a precise error.
        input(
            e.as_mut(),
            Input::SetSwitch {
                index: sw - 1,
                on: true,
            },
        )
        .unwrap();
        input(
            e.as_mut(),
            Input::SetButton {
                index: btn - 1,
                pressed: true,
            },
        )
        .unwrap();
        assert!(
            e.board().switches[sw - 1] && e.board().buttons[btn - 1],
            "{name}"
        );
        let err = input(
            e.as_mut(),
            Input::SetSwitch {
                index: sw,
                on: true,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            format!("switch index {sw} out of range 0..{sw}"),
            "{name}"
        );
        let err = input(
            e.as_mut(),
            Input::SetButton {
                index: btn,
                pressed: true,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            format!("button index {btn} out of range 0..{btn}"),
            "{name}"
        );
    }
}

#[test]
fn zedboard_leds_follow_the_switches_and_pin_names_come_from_the_board() {
    let mut e = engine_for("zedboard");
    for i in [0, 3, 6] {
        input(e.as_mut(), Input::SetSwitch { index: i, on: true }).unwrap();
    }
    let leds: Vec<u8> = e.board().leds.iter().map(|d| u8::from(*d != 0)).collect();
    // LED0..LED6 mirror SW0..SW6 on the demo engine; LED7 is its heartbeat (off at start).
    assert_eq!(leds, vec![1, 0, 0, 1, 0, 0, 1, 0]);
    let pin = |e: &mut dyn Engine, n: &str| match e.query(&Query::Pin { name: n.into() }, false) {
        Ok(Response::Pin { level, .. }) => level.symbol().to_string(),
        Ok(r) => panic!("{r:?}"),
        Err(x) => format!("{}|{}", x.message, x.hint.unwrap_or_default()),
    };
    assert_eq!(pin(e.as_mut(), "sw6"), "1");
    assert_eq!(pin(e.as_mut(), "led6"), "1");
    assert_eq!(pin(e.as_mut(), "led7"), "0");
    assert_eq!(pin(e.as_mut(), "btn6"), "0");
    assert_eq!(pin(e.as_mut(), "JD7"), "Z");
    // Zybo-only names are unknown here, and the hint lists this board's.
    let bad = pin(e.as_mut(), "led5_g");
    assert!(
        bad.contains("no pin or control named `led5_g`")
            && bad.contains("try sw0..sw7, btn0..btn6, led0..led7, JA1..JD10"),
        "{bad}"
    );
}

#[test]
fn a_recording_carries_a_custom_board_and_replays_without_the_file() {
    let file = examples().join("custom-board/project.toml");
    let p = Project::load(&file).unwrap();
    let mut e = p.build_engine(file.parent().unwrap()).unwrap();
    assert_eq!(e.def().name, "lab-board");
    e.command(Command::Run {
        duration_ps: 2_000_000_000,
    })
    .unwrap();
    input(e.as_mut(), Input::SetSwitch { index: 2, on: true }).unwrap();
    e.command(Command::Run {
        duration_ps: 5_000_000_000,
    })
    .unwrap();
    let rec = match e.query(&Query::Recording, false).unwrap() {
        Response::Recording(r) => *r,
        r => panic!("{r:?}"),
    };
    assert_eq!(rec.config.board, "lab-board");
    assert!(
        rec.config.board_def.is_some(),
        "the board travels with the recording"
    );

    // Through JSON and into a simulator that has never seen the board file.
    let text = serde_json::to_string(&rec).unwrap();
    assert!(
        !text.contains("svg_source"),
        "the artwork text is not recorded"
    );
    let back: Recording = serde_json::from_str(&text).unwrap();
    let (replayed, report) = Simulator::replay(&back).unwrap();
    assert!(report.matches, "{report:?}");
    assert_eq!(replayed.def().switches.len(), 3);
    assert_eq!(replayed.board().switches, vec![false, false, true]);

    // A built-in board does not bloat the recording.
    let mut z = engine_for("zedboard");
    match z.query(&Query::Recording, false).unwrap() {
        Response::Recording(r) => assert!(r.config.board_def.is_none()),
        r => panic!("{r:?}"),
    }
    assert_eq!(SimConfig::default().board, "zybo-z7-20");
}

// ---- the ZedBoard through a real RTL testbench (QEMU + XSim) -----------------------------

fn tools_present() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    let qemu = std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some();
    qemu && sim_api::find_vivado().is_some() && sim_api::find_dpi_lib().is_some()
}

fn uart1(e: &mut dyn Engine) -> String {
    match e
        .query(
            &Query::UartRead {
                port: 1,
                from: 0,
                max: 1 << 16,
            },
            false,
        )
        .unwrap()
    {
        Response::Uart { data, .. } => String::from_utf8_lossy(&data).into_owned(),
        r => panic!("{r:?}"),
    }
}

fn led_bits(e: &dyn Engine) -> u32 {
    e.board()
        .leds
        .iter()
        .enumerate()
        .fold(0, |a, (i, d)| a | (u32::from(*d >= 128) << i))
}

#[test]
fn zedboard_drives_eight_leds_from_firmware_and_takes_eight_switches_and_seven_buttons() {
    if !tools_present() {
        eprintln!("SKIPPED: needs the Xilinx QEMU, Vivado XSim and the DPI library");
        return;
    }
    let file = examples().join("zedboard-demo/project.toml");
    let mut e = Project::load(&file)
        .unwrap()
        .build_engine(file.parent().unwrap())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(e.def().name, "zedboard");
    assert!(e.status(false).capabilities.contains(&Capability::BoardIo));
    let run_ms = |e: &mut dyn Engine, ms: u64| {
        e.command(Command::Run {
            duration_ps: ms * 1_000_000_000,
        })
        .unwrap();
    };

    // The firmware writes 0xA5 to the eight LEDs: LEDs 0, 2, 5 and 7, the last of them
    // beyond the Zybo's four.
    run_ms(e.as_mut(), 3);
    let u = uart1(e.as_mut());
    assert!(u.contains("LED=0xa5"), "{u}");
    assert_eq!(led_bits(e.as_ref()), 0xA5);

    // Switches 4 and 7 reach the testbench's upper bits and come back on the LEDs.
    input(e.as_mut(), Input::SetSwitch { index: 4, on: true }).unwrap();
    input(e.as_mut(), Input::SetSwitch { index: 7, on: true }).unwrap();
    run_ms(e.as_mut(), 3);
    let u = uart1(e.as_mut());
    assert!(u.contains("SW=0x90"), "{u}");
    assert_eq!(led_bits(e.as_ref()), 0x90);

    // Button 6 is the seventh and last button.
    input(
        e.as_mut(),
        Input::SetButton {
            index: 6,
            pressed: true,
        },
    )
    .unwrap();
    run_ms(e.as_mut(), 3);
    let u = uart1(e.as_mut());
    assert!(
        u.contains("BTN=0x40") && u.contains("RESULT: ALL PASS"),
        "{u}"
    );
    assert_eq!(led_bits(e.as_ref()), 0x40);
}
