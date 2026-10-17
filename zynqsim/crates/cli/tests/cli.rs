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
    let x = zynqsim(&["init", d.to_str().unwrap(), "--filelist", "files.f"]);
    assert_eq!(x.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&x.stderr).contains("milestone P3"));

    let p = d.join("qemu.toml");
    std::fs::write(
        &p,
        "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\npl = \"icarus\"\n",
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
    assert_eq!(
        implemented,
        [
            "demo",
            "qemu",
            "native",
            "demo",
            "native",
            "verilator",
            "xsim"
        ]
    );
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

/// An IP with an AXI4 master, a slave and an interrupt: the generated testbench
/// runs the two-core DMA regression and produces *the same trace digest* as
/// the hand-written testbench of `examples/dma-regression`.
#[test]
fn init_ip_generates_the_dma_testbench_with_a_memory_bridge() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("init-ip-dma");
    let ip = fixtures().join("rtl/dma/dma_xform.v");
    let o = zynqsim(&[
        "--json",
        "init",
        d.to_str().unwrap(),
        "--ip",
        ip.to_str().unwrap(),
        "--elf",
        fixtures().join("elf/dma_test.elf").to_str().unwrap(),
        "--elf1",
        fixtures().join("elf/dma_cpu1.elf").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["axi_master"], "m_");
    assert_eq!(v["interrupts"], serde_json::json!(["irq"]));

    let run = |project: &str| -> Value {
        let o = zynqsim(&["--json", "run", "--project", project, "--for", "3ms"]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{} {}",
            stdout(&o),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_str(&stdout(&o)).unwrap()
    };
    let generated = run(d.join("project.toml").to_str().unwrap());
    let uart = generated["uart"]["1"].as_str().unwrap();
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert!(uart.matches("PASS ").count() >= 6, "{uart}");

    let hand = run(&example("dma-regression"));
    assert_eq!(generated["bus_accesses"], hand["bus_accesses"]);
    assert_eq!(
        generated["trace_hash"], hand["trace_hash"],
        "generated and hand-written testbenches behave identically"
    );
}

// -------------------------------------------------------------- Pmod OLED

/// Expected panel pixels of the OLED demo: the four lines of text in the
/// shared 5x7 font, upright (the firmware compensates the remap), 6 columns
/// per character, one 8-pixel page per line.
fn oled_expected() -> Vec<bool> {
    let lines = [
        "ZYNQSIM OLED",
        "HELLO FROM RUST",
        "PS PL 100 MHZ",
        "RESULT: PASS",
    ];
    let mut px = vec![false; 128 * 32];
    for (page, line) in lines.iter().enumerate() {
        for (i, c) in line.bytes().enumerate() {
            let g = sim_api::font5x7::glyph(c);
            for (cx, col) in g.iter().enumerate() {
                for bit in 0..7 {
                    if col >> bit & 1 != 0 {
                        px[(page * 8 + bit) * 128 + i * 6 + cx] = true;
                    }
                }
            }
        }
    }
    px
}

/// Firmware -> AXI-Lite -> SPI master RTL -> JA pins -> host SSD1306 model:
/// the picture is exactly the text the firmware drew.
#[test]
fn oled_demo_draws_text_pixel_exact() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let d = temp("oled");
    let png = d.join("oled.png");
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &example("oled-demo"),
        "--for",
        "4ms",
        "--device-out",
        &format!("JA={}", png.display()),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v["uart"]["1"]
            .as_str()
            .unwrap()
            .contains("RESULT: ALL PASS")
    );
    assert_eq!(v["devices"][0]["kind"], "oled");
    assert!(
        v["devices"][0]["description"]
            .as_str()
            .unwrap()
            .contains("512 data byte(s)"),
        "{v}"
    );

    let (w, h, rgb) = sim_api::board::decode_png_rgb(&std::fs::read(&png).unwrap()).unwrap();
    assert_eq!((w, h), (128, 32));
    let want = oled_expected();
    let mut bad = 0;
    for y in 0..32usize {
        for x in 0..128usize {
            let lit = rgb[(y * 128 + x) * 3 + 2] > 128;
            if lit != want[y * 128 + x] {
                bad += 1;
            }
        }
    }
    assert_eq!(bad, 0, "pixels differ from the expected text");
    assert!(want.iter().filter(|p| **p).count() > 150, "text has pixels");
}

/// RGB LEDs: six PWM pins reported by the testbench become duty cycles.
#[test]
fn rgb_demo_averages_the_pwm_pins_into_colours() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &example("rgb-demo"),
        "--for",
        "2ms",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v["uart"]["1"]
            .as_str()
            .unwrap()
            .contains("RESULT: ALL PASS")
    );
    let want = [[64i64, 128, 255], [255, 0, 32]];
    for (led, w) in want.iter().enumerate() {
        for (ch, d) in w.iter().enumerate() {
            let got = v["rgb"][led][ch].as_i64().unwrap();
            assert!(
                (got - d).abs() <= 4,
                "LED {led} channel {ch}: duty {got}, wanted about {d}: {}",
                v["rgb"]
            );
        }
    }
}

// ----------------------------------------------------- Pmod sensors (I2C, SPI)

/// Runs a sensor example and returns the parsed `--json` summary.
fn run_sensor(example_name: &str, duration: &str, extra: &[&str]) -> Value {
    let project = example(example_name);
    let mut args = vec!["--json", "run", "--project", &project, "--for", duration];
    args.extend_from_slice(extra);
    let o = zynqsim(&args);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_str(&stdout(&o)).unwrap()
}

/// The UART text of a run summary.
fn uart1(v: &Value) -> &str {
    v["uart"]["1"].as_str().unwrap()
}

/// The `values[name]` of the device on the first port in a run summary.
fn device_value(v: &Value, name: &str) -> f64 {
    v["device_state"][0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["name"] == name)
        .unwrap_or_else(|| panic!("no quantity {name} in {v}"))["value"]
        .as_f64()
        .unwrap()
}

/// Firmware -> AXI-Lite -> I2C master RTL -> JA pins -> host ADT7420 model and
/// back: what the firmware read is what the host set, in both resolutions and
/// at three bus clocks; the run is deterministic.
#[test]
fn tmp2_demo_reads_the_host_set_temperature_over_i2c() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();

    // 23.5 degC from project.toml, changed to 80 degC at 6 ms by the host.
    let args = ["--device-set", "6ms@JA.temperature=80"];
    let a = run_sensor("tmp2-demo", "12ms", &args);
    let text = uart1(&a);
    assert!(text.contains("ID 0xCB\r\n"), "{text}");
    for div in [250, 25, 2] {
        assert!(
            text.contains(&format!("BUS div={div} id=0xCB raw=0x0BC0\r\n")),
            "{text}"
        );
    }
    assert!(text.contains("T13 raw=0x0BC0 mC=23500\r\n"), "{text}");
    assert!(text.contains("T16 raw=0x0BC0 mC=23500\r\n"), "{text}");
    assert!(
        text.contains("T16 raw=0x2800 mC=80000\r\n"),
        "the value set at run time reaches the firmware: {text}"
    );
    assert_eq!(device_value(&a, "temperature"), 80.0);
    let regs = a["device_state"][0]["registers"].as_array().unwrap();
    let reg = |n: &str| regs.iter().find(|r| r["name"] == n).unwrap()["value"].as_u64();
    assert_eq!(reg("TEMP"), Some(0x2800));
    assert_eq!(
        reg("CONFIG"),
        Some(0x80),
        "the firmware selected 16-bit mode"
    );
    assert_eq!(reg("ID"), Some(0xCB));

    // Same inputs, same outputs: the whole trace has the same digest.
    let b = run_sensor("tmp2-demo", "12ms", &args);
    assert_eq!(a["trace_hash"], b["trace_hash"]);
    assert_eq!(a["trace_len"], b["trace_len"]);
    assert_eq!(uart1(&a), uart1(&b));

    // A negative start value from the command line (two's complement).
    let c = run_sensor(
        "tmp2-demo",
        "8ms",
        &["--device-set", "JA.temperature=-12.25"],
    );
    let text = uart1(&c);
    assert!(text.contains("T13 raw=0xF9E0 mC=-12250\r\n"), "{text}");
    assert!(text.contains("T16 raw=0xF9E0 mC=-12250\r\n"), "{text}");
}

/// SPI mode 0, MISO only: the 16-bit frame is `code << 4`.
#[test]
fn als_demo_reads_the_host_set_light_over_spi() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let v = run_sensor("als-demo", "4ms", &["--device-set", "2ms@JA.light=20"]);
    let text = uart1(&v);
    for div in [50, 5, 1] {
        assert!(
            text.contains(&format!("SPI div={div} frame=0x0AD0\r\n")),
            "{text}"
        );
    }
    assert!(text.contains("LIGHT frame=0x0AD0 code=173\r\n"), "{text}");
    assert!(
        text.contains("LIGHT frame=0x0140 code=20\r\n"),
        "the value set at run time reaches the firmware: {text}"
    );
    assert_eq!(device_value(&v, "light"), 20.0);
    assert_eq!(v["devices"][0]["kind"], "als");
}

/// Firmware -> AXI-Lite -> one PL block -> JA (a Pmod 8LD: the firmware's LED
/// patterns) and JB (a Pmod BTN: buttons and switches the host sets at run
/// time). The LEDs end on the last pattern the firmware wrote, the firmware
/// reads the host's values, and the run is deterministic.
#[test]
fn pmod_simple_demo_lights_the_8ld_and_reads_the_buttons() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let args = [
        "--device-set",
        "3ms@JB.btn0=1",
        "--device-set",
        "3ms@JB.sw2=1",
    ];
    let a = run_sensor("pmod-simple-demo", "40ms", &args);
    let text = uart1(&a);
    assert!(text.contains("LEDS 0xA5\r\n"), "{text}");
    assert!(
        text.contains("BTN 0x00\r\nBTN 0x41\r\n"),
        "the values set by the host reach the firmware: {text}"
    );
    assert!(text.contains("LEDS 0x3C\r\nRESULT: ALL PASS\r\n"), "{text}");

    // device_state[0] is the 8LD: it shows the last pattern written.
    assert_eq!(a["devices"][0]["kind"], "8ld");
    let leds = &a["device_state"][0]["registers"][0];
    assert_eq!(leds["name"], "LEDS");
    assert_eq!(leds["value"], 0x3C, "{a}");
    // Before the second pattern the first one is on the pins.
    let early = run_sensor("pmod-simple-demo", "2ms", &[]);
    assert_eq!(early["device_state"][0]["registers"][0]["value"], 0xA5);
    // device_state[1] is the BTN: the host's buttons and switches.
    let btn = &a["device_state"][1]["registers"];
    assert_eq!(btn[0]["value"], 0x01, "BUTTONS: btn0");
    assert_eq!(btn[1]["value"], 0x04, "SWITCHES: sw2");

    // Same inputs, same outputs.
    let b = run_sensor("pmod-simple-demo", "40ms", &args);
    assert_eq!(a["trace_hash"], b["trace_hash"]);
    assert_eq!(uart1(&a), uart1(&b));
}

/// SPI mode 3 with register read/write and a multi-byte burst.
#[test]
fn acl_demo_reads_the_host_set_acceleration_over_spi() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    let v = run_sensor("acl-demo", "4ms", &["--device-set", "2ms@JA.accel_z=-1"]);
    let text = uart1(&v);
    assert!(text.contains("ID 0xE5\r\n"), "{text}");
    assert!(
        text.contains("CFG power=0x08 format=0x08 rate=0x0A\r\n"),
        "{text}"
    );
    assert!(text.contains("ACC x=500 y=-250 z=1000 mg\r\n"), "{text}");
    assert!(
        text.contains("ACC x=500 y=-250 z=-1000 mg\r\n"),
        "the value set at run time reaches the firmware: {text}"
    );
    assert_eq!(device_value(&v, "accel_x"), 0.5);
    assert_eq!(device_value(&v, "accel_z"), -1.0);
}

/// Bad `--device-set` values are rejected before anything starts.
#[test]
fn device_set_arguments_are_checked() {
    for (spec, want) in [
        ("temperature=1", "expected PORT.NAME"),
        ("JA.temperature", "expected [TIME@]PORT.NAME=VALUE"),
        ("JZ.temperature=1", "not a Pmod port"),
        ("JA.temperature=warm", "not a number"),
    ] {
        let o = zynqsim(&["run", "--for", "1ms", "--device-set", spec]);
        assert_eq!(o.status.code(), Some(2), "{spec}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(want), "{spec}: {err}");
    }
    // A device that is not there (the demo engine has no PL pins) is refused
    // with the backend's explanation, not ignored.
    let o = zynqsim(&["run", "--for", "1ms", "--device-set", "JA.temperature=1"]);
    assert_eq!(o.status.code(), Some(3));
}

// -------------------------------------------------------------- Pcam (OV5640)

/// The Pcam's SCCB interface and frames end to end: power gating, chip ID,
/// reset bits, and whole captured frames in DDR for every streaming state.
/// The MIPI CSI-2 link is not simulated (docs/adr/010-pcam.md); the frames
/// reach the PL as a pixel bus through the video input.
#[test]
fn pcam_demo_gates_frames_on_the_streaming_registers() {
    if !qemu_available() || !xsim_available() {
        eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
        return;
    }
    ensure_dpi_lib();
    const W: u32 = 160;
    const H: u32 = 120;
    let d = temp("pcam");
    let cam_png = d.join("cam.png");
    let v = run_sensor(
        "pcam-demo",
        "40ms",
        &["--device-out", &format!("CAM={}", cam_png.display())],
    );
    let text = uart1(&v);
    assert!(text.contains("RESULT: ALL PASS"), "{text}");
    assert!(text.contains("POWER off ack=false\r\n"), "{text}");
    assert!(text.contains("POWER on ack=true\r\n"), "{text}");
    assert!(text.contains("CHIP ID 0x5640\r\n"), "{text}");
    assert!(
        text.contains("SYS_CTRL0 0x42\r\n"),
        "stopped at power on: {text}"
    );
    assert!(
        text.contains("RESET SYS_CTRL0 0x02\r\n"),
        "the reset bit clears itself: {text}"
    );

    // What each captured frame must be.
    const BARS: [[u32; 3]; 8] = [
        [255, 255, 255],
        [255, 255, 0],
        [0, 255, 255],
        [0, 255, 0],
        [255, 0, 255],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 0],
    ];
    let word = |c: [u32; 3]| (c[0] << 16) | (c[1] << 8) | c[2];
    let frame = |f: &dyn Fn(u32, u32) -> u32| {
        word_checksum(
            (0..H)
                .flat_map(|y| (0..W).map(move |x| (x, y)))
                .map(|(x, y)| f(x, y)),
        )
    };
    let black = frame(&|_, _| 0);
    let bars = frame(&|x, _| word(BARS[(x * 8 / W) as usize]));
    let squares = frame(&|x, y| word(BARS[((x * 8 / W + y * 8 / H) % 8) as usize]));
    let host = frame(&|x, y| {
        // The gradient the project asks for (`[media] video_in`).
        (x * 255 / W) << 16 | (y * 255 / H) << 8 | ((x + y) * 255 / (W + H))
    });
    for (label, sum, n) in [
        ("stopped", black, 1),
        ("bars", bars, 2),
        ("squares", squares, 3),
        ("host", host, 4),
        ("gated", black, 5),
    ] {
        let want = format!("CAP {label} checksum {sum:#010x} frames={n}\r\n");
        assert!(text.contains(&want), "want `{want}` in {text}");
    }
    assert_ne!(bars, squares);
    assert_ne!(bars, host);

    // The model state and its picture.
    assert_eq!(v["devices"][0]["kind"], "pcam");
    assert_eq!(v["devices"][0]["port"], 5);
    let desc = v["devices"][0]["description"].as_str().unwrap();
    assert!(desc.contains("MIPI CSI-2 not simulated"), "{desc}");
    let regs = v["device_state"][0]["registers"].as_array().unwrap();
    let reg = |n: &str| regs.iter().find(|r| r["name"] == n).unwrap()["value"].as_u64();
    assert_eq!(reg("CHIP_ID_H"), Some(0x56));
    assert_eq!(reg("CHIP_ID_L"), Some(0x40));
    assert_eq!(reg("STREAM_CTRL"), Some(0x0F));
    assert_eq!(reg("TEST_PATTERN"), Some(0x00));
    let (w, h, _) = sim_api::board::decode_png_rgb(&std::fs::read(&cam_png).unwrap()).unwrap();
    assert_eq!((w, h), (128, 32));
}
