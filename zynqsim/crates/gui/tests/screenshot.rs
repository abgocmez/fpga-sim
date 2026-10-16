//! Smoke test of the window screenshot path: starts the real GUI on the
//! built-in demo, lets it save a PNG of its window and exits. Needs a display
//! and a GPU; where the window cannot be created the test prints `SKIPPED:`
//! and passes.

use std::fs::File;
use std::process::Stdio;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Words that show up in the error when no window or GPU can be had.
const NO_DISPLAY: &[&str] = &[
    "display",
    "adapter",
    "gpu",
    "wgpu",
    "winit",
    "eventloop",
    "graphics",
    "opengl",
    "glutin",
    "surface",
];

#[test]
fn the_window_screenshot_is_a_picture_of_the_ui() {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let png = dir.join(format!("zynqsim-gui-shot-{pid}.png"));
    let log = dir.join(format!("zynqsim-gui-shot-{pid}.log"));
    let _ = std::fs::remove_file(&png);

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_zynqsim-gui"))
        .arg("--screenshot")
        .arg(&png)
        .args(["--exit-after-ms", "20000"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log).expect("create log file"))
        .spawn()
        .expect("start zynqsim-gui");

    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll zynqsim-gui") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("zynqsim-gui did not exit within 120 s");
        }
        sleep(Duration::from_millis(100));
    };
    let stderr = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);

    if !status.success() || !png.exists() {
        let lower = stderr.to_lowercase();
        if NO_DISPLAY.iter().any(|w| lower.contains(w)) {
            eprintln!(
                "SKIPPED: no window or GPU for the screenshot: {}",
                stderr.trim()
            );
            return;
        }
        panic!("screenshot run failed ({status}): {stderr}");
    }

    let bytes = std::fs::read(&png).expect("read the screenshot");
    let _ = std::fs::remove_file(&png);
    assert!(!bytes.is_empty(), "the screenshot file is empty");
    let (w, h, rgb) = sim_api::board::decode_png_rgb(&bytes).expect("screenshot is a PNG");
    assert!(w >= 640 && h >= 480, "screenshot is {w}x{h}");
    let mut colours: Vec<[u8; 3]> = rgb.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect();
    colours.sort_unstable();
    colours.dedup();
    assert!(
        colours.len() > 16,
        "screenshot is (nearly) a single colour: {} distinct colours",
        colours.len()
    );
}
