//! Audio codec and HDMI models against real XSim, with no PS: the test plays
//! the PS by driving the `PlTarget` API (register accesses, DMA served from a
//! fake DDR) and checks what the host receives.
//!
//! Needs Vivado XSim; prints `SKIPPED:` otherwise.

use backend_rtl_xsim::{XsimConfig, XsimPl};
use pl_api::media::SAMPLE_RATE;
use pl_api::{
    AudioSource, BusError, BusResult, MediaEvent, PlTarget, PsMasterPort, PsPort, VideoSource,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn find_vivado() -> Option<PathBuf> {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return None;
    }
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
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // Always rebuild: the DPI library changes with the source tree.
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
    lib
}

fn start(tag: &str) -> Option<XsimPl> {
    let vivado = find_vivado()?;
    let fx = workspace().join("fixtures").join("rtl").join("media");
    let work = std::env::temp_dir().join(format!("zynqsim-media-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let cfg = XsimConfig {
        vivado_root: vivado,
        sources: vec![
            fx.join("zs_media.sv"),
            fx.join("audio_ctl.v"),
            fx.join("video_fb.v"),
            fx.join("cosim_media.sv"),
        ],
        top: "cosim_media".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work,
        waves: false,
        connect_timeout: Duration::from_secs(300),
        cache_dir: None,
    };
    Some(XsimPl::start(&cfg).unwrap_or_else(|e| panic!("XSim: {e}")))
}

/// A PS with a DDR: PL masters read and write this map.
#[derive(Default)]
struct Ddr {
    mem: HashMap<u64, u8>,
    reads: u64,
    writes: u64,
}

impl Ddr {
    fn write_word(&mut self, addr: u64, v: u32) {
        for (i, b) in v.to_le_bytes().iter().enumerate() {
            self.mem.insert(addr + i as u64, *b);
        }
    }
    fn read_word(&self, addr: u64) -> u32 {
        let mut b = [0u8; 4];
        for (i, x) in b.iter_mut().enumerate() {
            *x = self.mem.get(&(addr + i as u64)).copied().unwrap_or(0);
        }
        u32::from_le_bytes(b)
    }
}

impl PsPort for Ddr {
    fn mem_read(&mut self, _p: PsMasterPort, addr: u64, buf: &mut [u8]) -> BusResult<()> {
        self.reads += 1;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.mem.get(&(addr + i as u64)).copied().unwrap_or(0);
        }
        Ok(())
    }
    fn mem_write(&mut self, _p: PsMasterPort, addr: u64, data: &[u8]) -> BusResult<()> {
        self.writes += 1;
        for (i, b) in data.iter().enumerate() {
            self.mem.insert(addr + i as u64, *b);
        }
        Ok(())
    }
    fn irq(&mut self, _l: u32, _v: bool, _t: u64) -> BusResult<()> {
        Ok(())
    }
}

const AUDIO: u64 = 0x43C0_0000;
const VIDEO: u64 = 0x43C1_0000;

struct Sys {
    pl: XsimPl,
    ddr: Ddr,
    now: u64,
}

impl Sys {
    fn wr(&mut self, addr: u64, v: u32) {
        self.now += 100;
        self.pl
            .write(0, addr, &v.to_le_bytes(), self.now, &mut self.ddr)
            .unwrap_or_else(|e: BusError| panic!("write {addr:#x}: {e}"));
    }
    fn rd(&mut self, addr: u64) -> u32 {
        self.now += 100;
        let mut b = [0u8; 4];
        self.pl
            .read(0, addr, &mut b, self.now, &mut self.ddr)
            .unwrap_or_else(|e: BusError| panic!("read {addr:#x}: {e}"));
        u32::from_le_bytes(b)
    }
    /// Advances PL time by `us` microseconds (lock-step sync).
    fn run_us(&mut self, us: u64) {
        self.now += us * 1000;
        self.pl.advance(self.now, &mut self.ddr);
    }
}

#[test]
fn media_design_builds_and_answers() {
    let Some(pl) = start("build") else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let mut s = Sys {
        pl,
        ddr: Ddr::default(),
        now: 1000,
    };
    s.wr(AUDIO + 0x04, 0x1234_5678);
    assert_eq!(s.rd(AUDIO + 0x04), 0x1234_5678);
    assert_eq!(s.rd(VIDEO + 0x18), (240 << 16) | 320, "video SIZE register");
}

// ------------------------------------------------------------------- audio

impl Sys {
    fn i2c(&mut self, scl: u32, sda: u32) {
        self.wr(AUDIO + 0x1C, scl | (sda << 1));
    }

    /// Bit-bangs one byte, MSB first, and returns whether the slave ACKed.
    fn i2c_byte(&mut self, byte: u8) -> bool {
        for i in (0..8).rev() {
            let bit = u32::from((byte >> i) & 1);
            self.i2c(0, bit);
            self.i2c(1, bit);
            self.i2c(0, bit);
        }
        self.i2c(0, 1); // release SDA for the ACK slot
        self.i2c(1, 1);
        let ack = self.rd(AUDIO + 0x1C) & 0x100 == 0; // slave pulls SDA low
        self.i2c(0, 1);
        ack
    }

    /// One SSM2603 register write: `{reg[6:0], value[8]} {value[7:0]}`.
    fn codec_write(&mut self, reg: u8, value: u16) -> bool {
        self.i2c(1, 1);
        self.i2c(1, 0); // START: SDA falls while SCL high
        self.i2c(0, 0);
        let a = self.i2c_byte(0x1A << 1);
        let b = self.i2c_byte((reg << 1) | ((value >> 8) as u8 & 1));
        let c = self.i2c_byte(value as u8);
        self.i2c(0, 0);
        self.i2c(1, 0);
        self.i2c(1, 1); // STOP: SDA rises while SCL high
        a && b && c
    }
}

/// Left-channel samples from audio events, in order, with their frame index.
fn left_samples(events: &[MediaEvent]) -> Vec<(u64, i16)> {
    let mut out = Vec::new();
    for e in events {
        if let MediaEvent::Audio {
            first_frame,
            samples,
            ..
        } = e
        {
            for (i, f) in samples.chunks_exact(2).enumerate() {
                out.push((first_frame + i as u64, f[0]));
            }
        }
    }
    out
}

/// Frequency from rising zero crossings over the span of the samples.
fn estimate_hz(s: &[i16]) -> f64 {
    let mut crossings = Vec::new();
    for (i, w) in s.windows(2).enumerate() {
        if w[0] < 0 && w[1] >= 0 {
            crossings.push(i);
        }
    }
    if crossings.len() < 3 {
        return 0.0;
    }
    let span = (crossings[crossings.len() - 1] - crossings[0]) as f64;
    (crossings.len() - 1) as f64 * f64::from(SAMPLE_RATE) / span
}

fn peak(s: &[i16]) -> i32 {
    s.iter().map(|&x| i32::from(x).abs()).max().unwrap_or(0)
}

#[test]
fn codec_receives_a_tone_obeys_registers_and_feeds_the_adc() {
    let Some(pl) = start("audio") else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let mut s = Sys {
        pl,
        ddr: Ddr::default(),
        now: 1000,
    };

    // ---- bring the codec up over (bit-banged) I2C ----
    let init: [(u8, u16); 8] = [
        (15, 0x000), // reset
        (6, 0x000),  // all blocks powered
        (2, 0x079),  // left headphone volume, 0 dB
        (3, 0x079),
        (5, 0x000), // DAC unmuted
        (7, 0x002), // I2S, 16-bit
        (8, 0x000), // normal mode, 48 kHz
        (9, 0x001), // digital core active
    ];
    for (reg, v) in init {
        assert!(s.codec_write(reg, v), "no ACK writing R{reg}");
    }
    let regs: Vec<_> =
        s.pl.drain_media()
            .into_iter()
            .filter_map(|e| match e {
                MediaEvent::CodecRegister { reg, value, .. } => Some((reg, value)),
                _ => None,
            })
            .collect();
    let want: Vec<_> = init.iter().map(|&(r, v)| (r, v)).collect();
    assert_eq!(regs, want, "register writes seen by the codec model");

    // ---- 1 kHz triangle, amplitude 16000 ----
    let step = (1000.0 / f64::from(SAMPLE_RATE) * 4_294_967_296.0) as u32;
    s.wr(AUDIO + 0x04, step);
    s.wr(AUDIO + 0x08, 16000);
    s.wr(AUDIO, 0b011); // ENABLE | TONE_EN
    s.run_us(30_000);
    let frames = s.rd(AUDIO + 0x0C);
    assert!(frames > 1300, "PL sent {frames} frames in 30 ms");
    let ev = s.pl.drain_media();
    let left = left_samples(&ev);
    // Drop the first millisecond (start-up, partial block).
    let tone: Vec<i16> = left.iter().skip(48).map(|&(_, x)| x).collect();
    assert!(tone.len() > 1000, "{} samples", tone.len());
    let hz = estimate_hz(&tone);
    assert!(
        (hz - 1000.0).abs() < 25.0,
        "tone is {hz:.1} Hz; first samples {:?}",
        &tone[..80.min(tone.len())]
    );
    let pk = peak(&tone);
    assert!((15000..=16100).contains(&pk), "peak {pk}");
    // Frame indices are consecutive: nothing was lost between blocks.
    assert!(left.windows(2).all(|w| w[1].0 == w[0].0 + 1));

    // ---- headphone volume -6 dB halves the amplitude ----
    assert!(s.codec_write(2, 0x079 - 6));
    assert!(s.codec_write(3, 0x079 - 6));
    s.pl.drain_media();
    s.run_us(6_000);
    // The DPI sends 64-frame blocks, so up to 63 frames produced before the
    // change are still in flight: measure after them.
    let half: Vec<i16> = left_samples(&s.pl.drain_media())
        .iter()
        .skip(130)
        .map(|&(_, x)| x)
        .collect();
    let pk6 = peak(&half);
    assert!(
        (7500..=8300).contains(&pk6),
        "-6 dB peak {pk6} (expected ~8000)"
    );

    // ---- digital mute ----
    assert!(s.codec_write(5, 0x008));
    s.pl.drain_media();
    s.run_us(6_000);
    let muted = left_samples(&s.pl.drain_media());
    assert!(muted.len() > 200);
    assert!(
        muted.iter().skip(130).all(|&(_, x)| x == 0),
        "samples after DACMU must be zero"
    );
    assert!(s.codec_write(5, 0x000));

    // ---- ADC: a 440 Hz tone at 0.5 on LINE IN, looped back to the DAC ----
    s.pl.set_media_sources(
        None,
        Some(AudioSource::Tone {
            freq_hz: 440.0,
            amplitude: 0.5,
        }),
    );
    s.wr(AUDIO, 0b101); // ENABLE | LOOP
    s.wr(AUDIO + 0x18, 0); // clear the peak
    s.pl.drain_media();
    s.run_us(30_000);
    let adc_peak = s.rd(AUDIO + 0x18);
    assert!(
        (15800..=16500).contains(&adc_peak),
        "ADC peak {adc_peak} (expected ~16384)"
    );
    let looped: Vec<i16> = left_samples(&s.pl.drain_media())
        .iter()
        .skip(130)
        .map(|&(_, x)| x)
        .collect();
    let hz = estimate_hz(&looped);
    assert!((hz - 440.0).abs() < 12.0, "looped-back tone is {hz:.1} Hz");
}

// ------------------------------------------------------------------- video

const W: usize = 320;
const H: usize = 240;
const FB: u64 = 0x0100_0000;
const CAP: u64 = 0x0200_0000;

fn test_pixel(x: usize, y: usize) -> u32 {
    let r = (x * 255 / W) as u32;
    let g = (y * 255 / H) as u32;
    let b = ((x ^ y) & 0xFF) as u32;
    (r << 16) | (g << 8) | b
}

/// Complete frames (all lines present) as `frame number -> rgb bytes`.
fn assemble(events: &[MediaEvent]) -> Vec<(u32, Vec<u8>)> {
    let mut lines: HashMap<u32, HashMap<u32, Vec<u8>>> = HashMap::new();
    let mut ended = Vec::new();
    for e in events {
        match e {
            MediaEvent::VideoLine { frame, y, rgb, .. } => {
                lines.entry(*frame).or_default().insert(*y, rgb.clone());
            }
            MediaEvent::VideoFrameEnd { frame, .. } => ended.push(*frame),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for f in ended {
        if let Some(l) = lines.get(&f) {
            if l.len() == H && (0..H as u32).all(|y| l.contains_key(&y)) {
                let mut rgb = Vec::with_capacity(W * H * 3);
                for y in 0..H as u32 {
                    rgb.extend_from_slice(&l[&y]);
                }
                out.push((f, rgb));
            }
        }
    }
    out
}

#[test]
fn framebuffer_scans_out_and_hdmi_input_is_captured() {
    let Some(pl) = start("video") else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let mut s = Sys {
        pl,
        ddr: Ddr::default(),
        now: 1000,
    };

    // ---- scan-out: a frame buffer written by the "PS" appears on HDMI TX ----
    for y in 0..H {
        for x in 0..W {
            s.ddr
                .write_word(FB + ((y * W + x) * 4) as u64, test_pixel(x, y));
        }
    }
    s.wr(VIDEO + 0x04, FB as u32);
    s.wr(VIDEO, 1); // SCAN_EN
    s.run_us(9_000); // a frame is ~3.7 ms
    let frames = assemble(&s.pl.drain_media());
    assert!(frames.len() >= 2, "complete frames: {}", frames.len());
    for (n, rgb) in &frames {
        let mut bad = 0;
        for y in 0..H {
            for x in 0..W {
                let want = test_pixel(x, y);
                let i = (y * W + x) * 3;
                let got = (u32::from(rgb[i]) << 16)
                    | (u32::from(rgb[i + 1]) << 8)
                    | u32::from(rgb[i + 2]);
                if got != want {
                    bad += 1;
                    assert!(
                        bad < 4,
                        "frame {n}: pixel ({x},{y}) = {got:#08x}, want {want:#08x}"
                    );
                }
            }
        }
        assert_eq!(bad, 0, "frame {n}");
    }
    assert!(s.rd(VIDEO + 0x10) >= 2, "SCAN_FRAMES");
    let reads = s.ddr.reads;
    assert!(
        reads > 20 * H as u64,
        "the engine fetched lines over AXI ({reads} bursts)"
    );

    // ---- capture: the host's image arrives in DDR through HDMI RX ----
    let src = VideoSource::gradient(W as u32, H as u32);
    s.pl.set_media_sources(Some(src.clone()), None);
    s.wr(VIDEO + 0x08, CAP as u32);
    s.wr(VIDEO, 0b11); // SCAN_EN | CAP_EN (arm)
    for _ in 0..20 {
        s.run_us(1_000);
        if s.rd(VIDEO + 0x0C) & 1 != 0 {
            break;
        }
    }
    assert_eq!(s.rd(VIDEO + 0x0C) & 1, 1, "CAP_DONE never set");
    assert_eq!(s.rd(VIDEO + 0x14), 1, "CAP_FRAMES");
    let mut bad = 0;
    for y in 0..H {
        for x in 0..W {
            let w = s.ddr.read_word(CAP + ((y * W + x) * 4) as u64);
            let i = (y * W + x) * 3;
            let want = (u32::from(src.rgb[i]) << 16)
                | (u32::from(src.rgb[i + 1]) << 8)
                | u32::from(src.rgb[i + 2]);
            if w & 0x00FF_FFFF != want {
                bad += 1;
                assert!(
                    bad < 4,
                    "captured pixel ({x},{y}) = {w:#010x}, want {want:#08x}"
                );
            }
        }
    }
    assert_eq!(bad, 0, "captured pixels differ from the source image");
}

// -------------------------------------------------- QEMU-style access pattern

/// What a CPU polling the frame counter looks like to the PL: single-register
/// reads every microsecond of simulated time, with syncs in between, while the
/// codec fetches and sends audio blocks. It must neither hang nor lose frames.
#[test]
fn polling_a_register_while_audio_streams_does_not_stall() {
    let Some(pl) = start("poll") else {
        eprintln!("SKIPPED: Vivado/XSim not found");
        return;
    };
    let mut s = Sys {
        pl,
        ddr: Ddr::default(),
        now: 1000,
    };
    s.pl.set_media_sources(
        None,
        Some(AudioSource::Tone {
            freq_hz: 440.0,
            amplitude: 0.5,
        }),
    );
    for (reg, v) in [
        (15u8, 0u16),
        (6, 0),
        (2, 0x79),
        (3, 0x79),
        (5, 0),
        (7, 2),
        (9, 1),
    ] {
        assert!(s.codec_write(reg, v));
    }
    s.wr(AUDIO + 0x04, 0x0089_705F);
    s.wr(AUDIO + 0x08, 12000);
    s.wr(AUDIO, 0b011);
    let mut last = 0u32;
    for _ in 0..3000 {
        s.run_us(1);
        let f = s.rd(AUDIO + 0x0C);
        assert!(f >= last, "frame counter went backwards: {last} -> {f}");
        last = f;
    }
    assert!(last > 100, "only {last} frames in 3 ms of polling");
    s.wr(AUDIO, 0b101); // loopback: ADC blocks are fetched from the host too
    for _ in 0..3000 {
        s.run_us(1);
        s.rd(AUDIO + 0x0C);
    }
    let n = left_samples(&s.pl.drain_media()).len();
    assert!(n > 200, "{n} DAC frames reached the host");
}
