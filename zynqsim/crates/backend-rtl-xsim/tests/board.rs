//! The README's "virtual Zybo" scenarios against real QEMU + real XSim:
//! interactive UART shell, LEDs driven by the RTL, switches and buttons
//! reaching the RTL, button -> IRQ_F2P[1] -> ISR -> LED toggle, and the DMA
//! image filter checked pixel by pixel against a Rust golden model.
//!
//! Needs Xilinx QEMU and Vivado XSim; prints `SKIPPED:` otherwise.

use backend_qemu::{QemuConfig, QemuMachine, RunEnd, UartMode};
use backend_rtl_xsim::{XsimConfig, XsimPl};
use pl_api::PlTarget;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const IMG_SRC: u64 = 0x0100_0000;
const IMG_DST: u64 = 0x0110_0000;
const PIXELS: usize = 64 * 64;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn find_qemu() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_QEMU") {
        return Some(PathBuf::from(p));
    }
    std::fs::read_dir(r"C:\Xilinx\Vitis")
        .ok()?
        .flatten()
        .map(|e| {
            e.path()
                .join("data")
                .join("emulation")
                .join("qemu_win")
                .join("qemu")
                .join("qemu-system-aarch64.exe")
        })
        .find(|p| p.is_file())
}

fn find_vivado() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_VIVADO") {
        return Some(PathBuf::from(p));
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(r"C:\Xilinx\Vivado")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("settings64.bat").is_file())
        .collect();
    v.sort();
    v.pop()
}

fn dpi_lib() -> PathBuf {
    let lib = workspace()
        .join("target")
        .join("dpi")
        .join("release")
        .join("libxsim_dpi.a");
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
                "--target-dir",
                "target/dpi",
            ])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success());
    }
    lib
}

fn work(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-board-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A running virtual board: QEMU (interactive UART) + the RTL.
struct Board {
    m: QemuMachine,
    pl: XsimPl,
    target_ns: u64,
    uart: String,
    leds: u8,
}

impl Board {
    fn start(tag: &str) -> Option<Board> {
        let (qemu, vivado) = (find_qemu()?, find_vivado()?);
        let dtb = QemuConfig::dtb_for(&qemu)?;
        let fx = workspace().join("fixtures");
        let mut q = QemuConfig::new(
            qemu,
            dtb,
            fx.join("elf/board_test.elf"),
            work(&format!("{tag}-q")),
        );
        q.uart = UartMode::Tcp;
        let x = XsimConfig {
            vivado_root: vivado,
            sources: vec![
                fx.join("rtl/dma/dma_xform.v"),
                fx.join("rtl/board/gpio_ctrl.v"),
                fx.join("rtl/board/cosim_top.sv"),
            ],
            top: "cosim_top".into(),
            defines: Vec::new(),
            dpi_lib: dpi_lib(),
            work_dir: work(&format!("{tag}-x")),
            waves: false,
            connect_timeout: Duration::from_secs(300),
            cache_dir: None,
        };
        let pl = XsimPl::start(&x).unwrap_or_else(|e| panic!("XSim: {e}"));
        let m = QemuMachine::spawn(&q).expect("QEMU starts");
        Some(Board {
            m,
            pl,
            target_ns: 0,
            uart: String::new(),
            leds: 0,
        })
    }

    /// Advances by `us` microseconds of virtual time.
    fn run_us(&mut self, us: u64) {
        self.target_ns += us * 1000;
        let end = self
            .m
            .run_until(self.target_ns, &mut self.pl, Duration::from_secs(300))
            .expect("run");
        assert_eq!(end, RunEnd::ReachedTime, "QEMU exited; uart: {}", self.uart);
        self.collect();
    }

    fn collect(&mut self) {
        // UART TCP bytes arrive asynchronously; give the reader thread a moment.
        std::thread::sleep(Duration::from_millis(20));
        self.uart
            .push_str(&String::from_utf8_lossy(&self.m.uart_take()));
        for o in self.pl.drain_board_outputs() {
            self.leds = o.leds;
        }
    }

    /// Runs until `needle` appears in the UART output after `from`, or fails.
    fn run_until_output(&mut self, from: usize, needle: &str, max_us: u64) {
        let mut spent = 0;
        while !self.uart[from..].contains(needle) {
            assert!(
                spent < max_us,
                "`{needle}` did not appear within {max_us} us; output since: {:?}",
                &self.uart[from..]
            );
            self.run_us(100);
            spent += 100;
        }
    }

    /// Types a shell line and returns what the shell printed in response, up
    /// to and including the next prompt.
    fn shell(&mut self, line: &str, max_us: u64) -> String {
        let from = self.uart.len();
        self.m.uart_write(format!("{line}\r").as_bytes()).unwrap();
        // The firmware prints "\r\n> " after finishing a command.
        self.run_until_output(from, "\r\n> ", max_us);
        let out = self.uart[from..].to_string();
        // Wait for the prompt that ends *this* command's output (the echo of
        // the typed line is followed by CR LF, then the result, then "> ").
        out
    }

    fn mem(&mut self, addr: u64, words: usize) -> Vec<u32> {
        self.m
            .read_memory(addr, words * 4)
            .expect("memory read")
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }
}

macro_rules! board_or_skip {
    ($tag:expr) => {
        match Board::start($tag) {
            Some(b) => b,
            None => {
                eprintln!("SKIPPED: needs the Xilinx QEMU and Vivado XSim");
                return;
            }
        }
    };
}

#[test]
fn shell_leds_switches_and_buttons() {
    let mut b = board_or_skip!("shell");

    // Boot: banner and prompt.
    b.run_until_output(0, "> ", 5_000);
    assert!(b.uart.contains("virtual Zybo Z7-20 ready"), "{}", b.uart);

    // `status` shows the power-on state.
    let out = b.shell("status", 5_000);
    assert!(out.contains("LED=0 SW=0 BTN=0"), "{out}");

    // `led 5`: the shell echoes the register, and the RTL's LED pins reach us.
    let out = b.shell("led 5", 5_000);
    assert!(out.contains("LED=5"), "{out}");
    b.run_us(50);
    assert_eq!(b.leds, 0b0101, "LED pins from the RTL");

    // Switches reach the RTL and are visible to the firmware.
    b.pl.set_board_inputs(0b0110, 0, b.m.now_ns());
    b.run_us(50);
    let out = b.shell("status", 5_000);
    assert!(out.contains("SW=6"), "{out}");

    // Button 2 pressed and released: GPIO event -> IRQ_F2P[1] -> ISR -> main
    // loop toggles LED2 (bit 2).
    let before = b.uart.len();
    b.pl.set_board_inputs(0b0110, 0b0100, b.m.now_ns());
    b.run_us(200);
    b.pl.set_board_inputs(0b0110, 0, b.m.now_ns());
    b.run_until_output(before, "[button event 0x4, total", 5_000);
    b.run_us(100);
    assert_eq!(
        b.leds, 0b0001,
        "LED2 toggled off by the button handler (was 0b0101)"
    );
    assert!(b.pl.stats().irqs >= 1, "{:?}", b.pl.stats());
    // The handler can run twice for one press: the IRQ is level-sensitive and
    // the PL's falling edge reaches QEMU one quantum after the W1C write, so the
    // GIC re-signals once before the line drops. That is the documented
    // quantum-granularity of PS/PL coupling, not a bug in the design; what is
    // required is that the event is delivered and handled at least once.
    let tail = &b.uart[before..];
    let total: u32 = tail
        .split("total ")
        .nth(1)
        .and_then(|t| t.split(']').next())
        .and_then(|n| n.trim().parse().ok())
        .expect("event count in the shell output");
    assert!(
        (1..=2).contains(&total),
        "button handler ran {total} times: {tail:?}"
    );

    // `end` stops the simulation from the firmware.
    let from = b.uart.len();
    b.m.uart_write(b"end\r").unwrap();
    b.run_until_output(from, "COSIM_DONE", 5_000);
    b.run_us(200);
    assert!(b.pl.finished(), "the testbench should have ended the run");
}

/// The README's "0 mismatches of 4096 pixels vs a golden model", in Rust.
#[test]
fn image_filters_match_the_golden_model_pixel_for_pixel() {
    let mut b = board_or_skip!("img");
    b.run_until_output(0, "> ", 5_000);

    // The disc image the firmware generates at boot, and the Rust model of it.
    let src = b.mem(IMG_SRC, PIXELS);
    for (i, &p) in src.iter().enumerate() {
        let (x, y) = ((i % 64) as i32, (i / 64) as i32);
        let d2 = (x - 32) * (x - 32) + (y - 32) * (y - 32);
        let want = if d2 > 900 { 20 } else { 255 - (d2 * 255) / 900 };
        assert_eq!(p, want as u32, "source pixel ({x},{y})");
    }

    // (command, scale, offset): `bright 60` = x*1+60, `contrast 3` = x*3-256,
    // `run 2 -100 1` = x*2-100, all clamped to 0..255.
    for (cmd, scale, off) in [
        ("bright 60", 1i64, 60i64),
        ("contrast 3", 3, -256),
        ("run 2 -100 1", 2, -100),
    ] {
        let out = b.shell(cmd, 20_000);
        assert!(out.contains("filtered 4096 pixels"), "{cmd}: {out}");
        let dst = b.mem(IMG_DST, PIXELS);
        let mut bad = 0;
        for (i, (&s, &d)) in src.iter().zip(&dst).enumerate() {
            let want = (i64::from(s) * scale + off).clamp(0, 255) as u32;
            if d != want {
                bad += 1;
                assert!(bad < 5, "{cmd}: pixel {i}: src {s}, got {d}, want {want}");
            }
        }
        assert_eq!(bad, 0, "{cmd}: mismatching pixels");
    }

    // Other test patterns regenerate the source image.
    b.shell("img 0", 20_000);
    let grad = b.mem(IMG_SRC, PIXELS);
    assert!(
        grad.iter()
            .enumerate()
            .all(|(i, &p)| p == ((i % 64) * 4) as u32)
    );
    b.shell("img 2", 20_000);
    let chk = b.mem(IMG_SRC, PIXELS);
    assert!(chk.iter().enumerate().all(|(i, &p)| {
        let (x, y) = (i % 64, i / 64);
        p == if ((x / 8) + (y / 8)) & 1 == 1 {
            200
        } else {
            40
        }
    }));
}
