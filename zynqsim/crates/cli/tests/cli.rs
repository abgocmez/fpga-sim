//! End-to-end tests of the `zynqsim` binary: output, files and exit codes.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn run_produces_expected_uart_and_a_stable_hash() {
    let args = [
        "--json",
        "run",
        "--for",
        "300ms",
        "--at",
        "10ms=sw0:on",
        "--at",
        "20ms=btn1:tap",
        "--at",
        "100ms=uart1:ping\\r\\n",
    ];
    let a = zynqsim(&args);
    assert_eq!(
        a.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&a.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&a)).unwrap();
    assert_eq!(v["backend"]["fidelity"], "demo");
    assert_eq!(
        v["uart"]["1"],
        "zynqsim demo ready\r\nBTN1 pressed\r\nping\r\n"
    );
    assert_eq!(v["leds"][0]["duty"], 255);
    assert_eq!(v["inputs"], 4); // sw0, btn1 down, btn1 up, uart

    // Determinism: a second process produces the identical digest.
    let b: Value = serde_json::from_str(&stdout(&zynqsim(&args))).unwrap();
    assert_eq!(v["trace_hash"], b["trace_hash"]);

    // A different seed changes it.
    let mut other = args.to_vec();
    other.extend(["--seed", "2"]);
    let c: Value = serde_json::from_str(&stdout(&zynqsim(&other))).unwrap();
    assert_ne!(v["trace_hash"], c["trace_hash"]);
}

#[test]
fn human_output_states_the_fidelity() {
    let o = zynqsim(&["run", "--for", "10ms"]);
    assert_eq!(o.status.code(), Some(0));
    let s = stdout(&o);
    assert!(s.contains("fidelity: demo"), "{s}");
    assert!(s.contains("no CPU or RTL is simulated"), "{s}");
}

#[test]
fn record_then_test_and_trace_round_trip_and_detect_tampering() {
    let d = temp("rec");
    let rec = d.join("s.json");
    let vcd = d.join("s.vcd");
    let o = zynqsim(&[
        "run",
        "--for",
        "1s",
        "--at",
        "5ms=sw1:on",
        "--at",
        "9ms=uart0:abc",
        "--record",
        rec.to_str().unwrap(),
        "--vcd",
        vcd.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        std::fs::read_to_string(&vcd)
            .unwrap()
            .contains("$var wire 8")
    );

    let t = zynqsim(&["test", rec.to_str().unwrap()]);
    assert_eq!(t.status.code(), Some(0));
    assert!(stdout(&t).starts_with("PASS"));

    // Changing the time of one input must make the replay differ: exit code 1.
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&rec).unwrap()).unwrap();
    v["entries"][0]["t_ps"] = Value::from(6_000_000_000u64);
    let bad = d.join("bad.json");
    std::fs::write(&bad, v.to_string()).unwrap();
    let t = zynqsim(&["test", bad.to_str().unwrap()]);
    assert_eq!(t.status.code(), Some(1));
    assert!(stdout(&t).starts_with("FAIL"));

    let perf = d.join("s.perfetto.json");
    let tr = zynqsim(&[
        "trace",
        rec.to_str().unwrap(),
        "--perfetto",
        perf.to_str().unwrap(),
    ]);
    assert_eq!(tr.status.code(), Some(0));
    let j: Value = serde_json::from_str(&std::fs::read_to_string(&perf).unwrap()).unwrap();
    assert!(j["traceEvents"].as_array().unwrap().len() > 10);
}

#[test]
fn init_creates_a_valid_project_and_refuses_to_overwrite() {
    let d = temp("init");
    let proj = d.join("mine");
    let o = zynqsim(&["init", proj.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    let file = proj.join("project.toml");
    assert!(file.is_file());

    let i = zynqsim(&["inspect", "--project", file.to_str().unwrap()]);
    assert_eq!(i.status.code(), Some(0), "{}", stdout(&i));
    let r = zynqsim(&["run", "--project", file.to_str().unwrap(), "--for", "5ms"]);
    assert_eq!(r.status.code(), Some(0));

    let again = zynqsim(&["init", proj.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&again.stderr).contains("refusing to overwrite"));
}

#[test]
fn unsupported_features_exit_3_with_a_clear_message() {
    let d = temp("uns");
    let x = zynqsim(&["init", d.to_str().unwrap(), "--xsa", "design.xsa"]);
    assert_eq!(x.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&x.stderr).contains("milestone P3"));

    let p = d.join("qemu.toml");
    std::fs::write(
        &p,
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"native\"\n",
    )
    .unwrap();
    let r = zynqsim(&["run", "--project", p.to_str().unwrap()]);
    assert_eq!(r.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&r.stderr).contains("not implemented in this build"));
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        vec!["run", "--at", "nonsense"],
        vec!["run", "--for", "10"],
        vec!["run", "--for", "1ms", "--at", "2ms=sw0:on"],
        vec!["test", "does-not-exist.json"],
        vec!["trace", "x.json"],
    ] {
        let o = zynqsim(&args);
        assert_eq!(
            o.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    // JSON mode reports errors as JSON on stdout.
    let o = zynqsim(&["--json", "run", "--for", "10"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["error"]["exit_code"], 2);
}

#[test]
fn inspect_lists_backends_without_claiming_unbuilt_ones() {
    let o = zynqsim(&["--json", "inspect"]);
    assert_eq!(o.status.code(), Some(0));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["board"]["part"], "xc7z020clg400-1");
    assert_eq!(v["board"]["pin_status"], "unverified");
    let backs = v["backends"].as_array().unwrap();
    let implemented: Vec<&str> = backs
        .iter()
        .filter(|b| b["implemented"] == true)
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(implemented, ["demo", "qemu", "demo", "native", "xsim"]);
}

/// Prompt §14.6: every integration test runs 5x with identical trace hashes.
#[test]
fn five_separate_processes_produce_one_trace_hash() {
    let args = [
        "--json",
        "run",
        "--for",
        "2s",
        "--seed",
        "11",
        "--at",
        "3ms=sw3:on",
        "--at",
        "40ms=btn2:tap",
        "--at",
        "50ms=uart0:determinism\r\n",
        "--at",
        "1500ms=reset:12",
    ];
    let hashes: std::collections::BTreeSet<String> = (0..5)
        .map(|_| {
            let o = zynqsim(&args);
            assert_eq!(o.status.code(), Some(0));
            let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
            v["trace_hash"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(hashes.len(), 1, "runs diverged: {hashes:?}");
}

fn qemu_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn math_project() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/math-qemu/project.toml")
        .to_string_lossy()
        .into_owned()
}

/// Real QEMU + real firmware + native PL, three separate processes, one digest.
#[test]
fn qemu_project_runs_the_firmware_and_is_deterministic() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let project = math_project();
    let run = || {
        let o = zynqsim(&["--json", "run", "--project", &project, "--for", "20ms"]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_str::<Value>(&stdout(&o)).unwrap()
    };
    let first = run();
    assert_eq!(first["backend"]["ps"], "qemu");
    assert_eq!(first["backend"]["pl"], "native");
    assert_eq!(first["backend"]["fidelity"], "f0");
    assert_eq!(first["bus_accesses"], 21);
    let uart = first["uart"]["1"].as_str().unwrap();
    assert_eq!(uart.matches("PASS in=").count(), 7, "{uart}");
    assert!(uart.contains("RESULT: ALL PASS") && uart.contains("COSIM_DONE"));
    for _ in 0..2 {
        let again = run();
        assert_eq!(again["trace_hash"], first["trace_hash"]);
        assert_eq!(again["now_ps"], first["now_ps"]);
    }
}

#[test]
fn qemu_backend_says_what_it_cannot_do() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let project = math_project();
    // Board inputs are not connected to anything.
    let o = zynqsim(&[
        "run",
        "--project",
        &project,
        "--for",
        "2ms",
        "--at",
        "1ms=sw0:on",
    ]);
    assert_eq!(o.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not connected"));
}

/// A QEMU session is recorded as a journal and replays to the same digest;
/// tampering with the digest or dropping the project is reported.
#[test]
fn qemu_sessions_record_and_replay_and_detect_tampering() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let project = math_project();
    let d = temp("qemu-rec");
    let rec = d.join("r.json");
    let o = zynqsim(&[
        "run",
        "--project",
        &project,
        "--for",
        "1ms",
        "--record",
        rec.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = std::fs::read_to_string(&rec).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["journal"].as_array().is_some_and(|j| !j.is_empty()),
        "{v}"
    );
    assert!(
        v["project"]
            .as_str()
            .is_some_and(|p| p.ends_with("project.toml"))
    );

    let o = zynqsim(&["test", rec.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert!(stdout(&o).contains("PASS"), "{}", stdout(&o));

    // The trace can be exported from the replayed session.
    let vcd = d.join("r.vcd");
    let o = zynqsim(&[
        "trace",
        rec.to_str().unwrap(),
        "--vcd",
        vcd.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert!(
        std::fs::read_to_string(&vcd)
            .unwrap()
            .contains("$timescale")
    );

    // A wrong digest fails the replay.
    let mut bad = v.clone();
    bad["trace_hash"] = Value::String("fnv1a64:0000000000000000".to_string());
    let badf = d.join("bad.json");
    std::fs::write(&badf, bad.to_string()).unwrap();
    let o = zynqsim(&["test", badf.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));

    // Without its project a QEMU recording cannot be replayed: say so.
    let mut orphan = v;
    orphan.as_object_mut().unwrap().remove("project");
    let orf = d.join("orphan.json");
    std::fs::write(&orf, orphan.to_string()).unwrap();
    let o = zynqsim(&["test", orf.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("project"));
}

fn xsim_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "xsim" && b["found"] == true)
}

/// Builds the freestanding DPI library if the working tree does not have it.
fn ensure_dpi_lib() {
    let lib =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/dpi/release/libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
            ])
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .status()
            .expect("cargo runs");
        assert!(st.success(), "building the DPI library failed");
    }
}

/// Prompt §14.1 + §14.6: the prototype DMA regression (two cores, RTL DMA in
/// XSim, IRQ), run as separate processes, identical trace digests.
#[test]
fn dma_regression_passes_and_is_deterministic_across_processes() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/dma-regression/project.toml")
        .to_string_lossy()
        .into_owned();
    let run = || {
        let o = zynqsim(&["--json", "run", "--project", &project, "--for", "3ms"]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_str::<Value>(&stdout(&o)).unwrap()
    };
    let first = run();
    assert_eq!(first["backend"]["pl"], "xsim");
    let uart = first["uart"]["1"].as_str().unwrap();
    for case in [
        "PASS polled-64",
        "PASS irq-1000",
        "PASS cross4k-200",
        "PASS single-word",
        "PASS pipeline",
        "PASS unaligned-src rejected",
        "RESULT: ALL PASS",
    ] {
        assert!(uart.contains(case), "missing `{case}` in {uart}");
    }
    assert!(!uart.contains("FAIL"), "{uart}");
    assert!(first["bus_accesses"].as_u64().unwrap() > 600);
    for _ in 0..2 {
        let again = run();
        assert_eq!(again["trace_hash"], first["trace_hash"]);
        assert_eq!(again["now_ps"], first["now_ps"]);
    }
}

fn board_project() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/board-demo/project.toml")
        .to_string_lossy()
        .into_owned()
}

/// Interactive shell, LEDs from the RTL, switches and buttons into it.
#[test]
fn board_demo_shell_leds_switches_and_button_interrupt() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &board_project(),
        "--for",
        "15ms",
        "--at",
        "3ms=uart1:led 5\\r",
        "--at",
        "6ms=sw1:on",
        "--at",
        "7ms=uart1:status\\r",
        "--at",
        "10ms=btn2:down",
        "--at",
        "11ms=btn2:up",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("virtual Zybo Z7-20 ready"), "{uart}");
    assert!(uart.contains("LED=5"), "{uart}");
    assert!(uart.contains("SW=2"), "switch 1 reached the RTL: {uart}");
    assert!(
        uart.contains("[button event 0x4"),
        "button IRQ handled: {uart}"
    );
    // `led 5` sets 0b0101 and the button handler toggles LED2. Typed UART input
    // is delivered on the host's schedule, not virtual time's, so on a loaded
    // machine the button can be handled first (then `led 5` leaves LED2 on):
    // both orders are correct, and LED1/LED3 are off either way.
    let duty = |i: usize| v["leds"][i]["duty"].as_u64().unwrap();
    assert_eq!((duty(0), duty(1), duty(3)), (255, 0, 0), "{v}");
    let toggled_after_led5 = duty(2) == 0 && uart.find("LED=5") < uart.find("[button event");
    let led5_after_toggle = duty(2) == 255 && uart.find("[button event") < uart.find("LED=5");
    assert!(
        toggled_after_led5 || led5_after_toggle,
        "LED2 {} vs order in {uart:?}",
        duty(2)
    );
}

/// "0 mismatches of 4096 pixels" against a golden model, in Rust, with the
/// images fetched through the CLI's memory dump.
#[test]
fn board_demo_image_filter_matches_golden_through_the_cli() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("img");
    let (src, dst, png) = (d.join("src.bin"), d.join("dst.bin"), d.join("dst.png"));
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &board_project(),
        "--for",
        "40ms",
        "--at",
        "3ms=uart1:img 2\\r",
        "--at",
        "8ms=uart1:contrast 3\\r",
        "--dump-mem",
        &format!("0x01000000,16384,{}", src.display()),
        "--dump-mem",
        &format!("0x01100000,16384,{}", dst.display()),
        "--dump-image",
        &format!("0x01100000,64x64,{}", png.display()),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(
        uart.contains("filtered 4096 pixels: out = in*3-256"),
        "{uart}"
    );

    let words = |p: &PathBuf| -> Vec<i64> {
        std::fs::read(p)
            .unwrap()
            .chunks_exact(4)
            .map(|b| i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            .collect()
    };
    let (s, t) = (words(&src), words(&dst));
    assert_eq!((s.len(), t.len()), (4096, 4096));
    // pattern 2 is the checkerboard of 200 / 40
    assert!(s.iter().all(|&p| p == 200 || p == 40));
    let bad = s
        .iter()
        .zip(&t)
        .filter(|(a, b)| (**a * 3 - 256).clamp(0, 255) != **b)
        .count();
    assert_eq!(bad, 0, "mismatching pixels");

    let png_bytes = std::fs::read(&png).unwrap();
    assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        &png_bytes[16..24],
        &[0, 0, 0, 64, 0, 0, 0, 64],
        "64x64 IHDR"
    );
}

// ------------------------------------------------------------ audio and video

fn example(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join("project.toml")
        .to_string_lossy()
        .into_owned()
}

/// Left-channel samples of a 16-bit stereo WAV written by the simulator.
fn wav_left(bytes: &[u8]) -> (u32, Vec<i16>) {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let at = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("data chunk");
    let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
    let left = bytes[at + 8..at + 8 + len]
        .chunks_exact(4)
        .map(|f| i16::from_le_bytes([f[0], f[1]]))
        .collect();
    (rate, left)
}

/// Frequency from rising zero crossings.
fn hz(samples: &[i16], rate: u32) -> f64 {
    let c: Vec<usize> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0 && w[1] >= 0)
        .map(|(i, _)| i)
        .collect();
    assert!(c.len() > 3, "too few zero crossings: {}", c.len());
    (c.len() - 1) as f64 * f64::from(rate) / (c[c.len() - 1] - c[0]) as f64
}

/// Rust firmware brings the SSM2603 up over I2C, plays 1 kHz, then loops the
/// host's 440 Hz LINE IN back; the host captures both from the DAC.
#[test]
fn audio_demo_plays_a_tone_and_loops_line_in_back() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("audio");
    let wav = d.join("out.wav");
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &example("audio-demo"),
        "--for",
        "90ms",
        "--audio-in",
        "tone:440:0.5",
        "--audio-out",
        &wav.to_string_lossy(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("8/8 register writes acknowledged"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(v["media"]["codec_writes"], 8, "{v}");

    let (rate, left) = wav_left(&std::fs::read(&wav).unwrap());
    assert_eq!(rate, 48_000);
    assert!(left.len() >= 2000, "{} frames", left.len());
    // Tone phase: 1 kHz at amplitude 16000 (triangle), dropping the first block.
    let tone = &left[64..1100];
    assert!((hz(tone, rate) - 1000.0).abs() < 30.0, "{}", hz(tone, rate));
    let pk = tone.iter().map(|&s| i32::from(s).abs()).max().unwrap();
    assert!((15_900..=16_100).contains(&pk), "tone peak {pk}");
    // Loopback phase: the 440 Hz LINE IN source at 0.5 (sine, 16384).
    let n = left.len();
    let looped = &left[n - 700..];
    assert!(
        (hz(looped, rate) - 440.0).abs() < 25.0,
        "{}",
        hz(looped, rate)
    );
    let pk = looped.iter().map(|&s| i32::from(s).abs()).max().unwrap();
    assert!((16_000..=16_500).contains(&pk), "loopback peak {pk}");
}

/// Firmware checksum of 0x00RRGGBB words (FNV-1a over words).
fn word_checksum(words: impl Iterator<Item = u32>) -> u32 {
    words.fold(0x811C_9DC5u32, |h, w| {
        (h ^ (w & 0x00FF_FFFF)).wrapping_mul(0x0100_0193)
    })
}

/// The firmware draws a test card and scans it out; the PL captures the PNG the
/// host feeds to HDMI RX. Both pictures are checked against Rust models.
#[test]
fn hdmi_demo_scans_out_a_frame_and_captures_the_host_image() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    const W: u32 = 320;
    const H: u32 = 240;
    let d = temp("hdmi");

    // A picture of our own for HDMI RX: a diagonal plaid, as PNG.
    let rx_px = |x: u32, y: u32| -> (u8, u8, u8) {
        (
            ((x * 3 + y) & 0xFF) as u8,
            ((y * 2) & 0xFF) as u8,
            ((x ^ (y << 1)) & 0xFF) as u8,
        )
    };
    let mut rgba = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let (r, g, b) = rx_px(x, y);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let rx_png = d.join("rx.png");
    std::fs::write(
        &rx_png,
        sim_api::board::encode_rgba_png(W, H, rgba).unwrap(),
    )
    .unwrap();
    let tx_png = d.join("tx.png");

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &example("hdmi-demo"),
        "--for",
        "40ms",
        "--video-in",
        &rx_png.to_string_lossy(),
        "--video-out",
        &tx_png.to_string_lossy(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = v["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert!(v["media"]["video_frames"].as_u64().unwrap() >= 2, "{v}");

    // The captured frame equals the picture the host supplied.
    let want_rx = word_checksum((0..H).flat_map(|y| {
        (0..W).map(move |x| {
            let (r, g, b) = rx_px(x, y);
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        })
    }));
    assert!(
        uart.contains(&format!("CAPTURE frames=1 checksum 0x{want_rx:08x}")),
        "want capture checksum {want_rx:08x}: {uart}"
    );

    // The scanned-out frame equals the firmware's test card.
    let card = |x: u32, y: u32| -> (u8, u8, u8) {
        (
            (x * 255 / W) as u8,
            (y * 255 / H) as u8,
            ((x ^ y) & 0xFF) as u8,
        )
    };
    let want_fb = word_checksum((0..H).flat_map(|y| {
        (0..W).map(move |x| {
            let (r, g, b) = card(x, y);
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        })
    }));
    assert!(
        uart.contains(&format!("FB 0x01000000 checksum 0x{want_fb:08x}")),
        "{uart}"
    );
    let mut rgba = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let (r, g, b) = card(x, y);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let want_png = sim_api::board::encode_rgba_png(W, H, rgba).unwrap();
    assert_eq!(
        std::fs::read(&tx_png).unwrap(),
        want_png,
        "HDMI TX picture differs from the test card"
    );
}

// ------------------------------------------------------------ init --ip

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// `init --ip` writes a testbench and project for an AXI4-Lite IP that run
/// the real firmware against the real RTL, with the same bus traffic as the
/// native model of the same IP.
#[test]
fn init_ip_generates_a_project_that_passes_with_the_real_firmware() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("init-ip");
    let ip = fixtures().join("rtl/math/my_math_ip.v");
    let elf = fixtures().join("elf/math_test.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        d.to_str().unwrap(),
        "--ip",
        ip.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["module"], "my_math_ip");
    assert_eq!(v["interface"], "S_AXI_");
    assert_eq!(v["addr_width"], 4);
    assert_eq!(v["elf_exists"], true);
    let tb = std::fs::read_to_string(d.join("tb/cosim_top.sv")).unwrap();
    assert!(tb.contains("my_math_ip dut ("));

    // Refuses to overwrite.
    let again = zynqsim(&["init", d.to_str().unwrap(), "--ip", ip.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(2));

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "3ms",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let r: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = r["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert_eq!(
        r["bus_accesses"], 21,
        "same traffic as the native model run"
    );
}

#[test]
fn init_ip_explains_what_is_wrong_with_an_unsuitable_module() {
    let d = temp("init-ip-bad");
    let src = d.join("not_axi.v");
    std::fs::write(
        &src,
        "module m (input clk, input [7:0] d, output [7:0] q);\nendmodule\n",
    )
    .unwrap();
    let o = zynqsim(&[
        "init",
        d.join("proj").to_str().unwrap(),
        "--ip",
        src.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("AXI4-Lite"), "{err}");
    assert!(
        !d.join("proj").join("project.toml").exists(),
        "nothing written"
    );

    // A missing module name is reported with the name.
    let o = zynqsim(&[
        "init",
        d.join("proj2").to_str().unwrap(),
        "--ip",
        src.to_str().unwrap(),
        "--module",
        "other",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("`other`"));
}

/// Interrupt outputs of the IP are forwarded to the PS: a generated testbench
/// raises IRQ_F2P[0], the Rust firmware's GIC setup and handler take it.
#[test]
fn init_ip_forwards_interrupts_to_a_firmware_handler() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("init-ip-irq");
    let ip = fixtures().join("rtl/irq/irq_ip.v");
    let elf = fixtures().join("elf/rust_irq.elf");
    let o = zynqsim(&[
        "--json",
        "init",
        d.to_str().unwrap(),
        "--ip",
        ip.to_str().unwrap(),
        "--elf",
        elf.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["interrupts"], serde_json::json!(["irq"]));

    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        d.join("project.toml").to_str().unwrap(),
        "--for",
        "4ms",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let r: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let uart = r["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("events=3 handled=3"), "{uart}");
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
}
