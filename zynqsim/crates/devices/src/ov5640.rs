//! The OmniVision OV5640 image sensor on a Digilent Pcam 5C: its SCCB control
//! interface, and the frames it delivers to the PL's video input.
//!
//! **What is and is not simulated.** The Pcam's pixels travel over MIPI
//! CSI-2 on a D-PHY. This model has no D-PHY and no CSI-2 packets: the PL's
//! video input (`zs_video_rx_*`, the same path as HDMI RX) *is* the sensor
//! output, as a pixel bus. The SCCB bus (I2C-compatible, 16-bit register
//! addresses) is modelled at the pin level, and the frames the PL receives are
//! gated by the sensor's streaming registers. A real design's MIPI CSI-2
//! receiver, its lane configuration (`0x300E`, `0x4800`) and the sensor's
//! clock tree (PLL registers) are accepted as plain registers and do nothing.
//!
//! Pin order of the group (the camera connector, group 5, `cam` in
//! `[devices]`): bit0 `CAM_GPIO` (power enable, driven by the PL), bit2 SCL,
//! bit3 SDA. `VERIFY:` the Zybo Z7 connector pin out.
//!
//! Modelled registers (`VERIFY:` all values and bit meanings recalled from the
//! OV5640 datasheet and the Linux driver, not checked against either):
//!
//! | address | name | behaviour |
//! |---|---|---|
//! | `0x300A`, `0x300B` | chip ID | read only, `0x56`, `0x40` |
//! | `0x3008` | SYSTEM_CTRL0 | bit7 software reset (self clearing, all registers return to their defaults), bit6 software power down; reset value `0x42` here (stopped) |
//! | `0x4202` | stream control | low nibble `0x0` streams, `0xF` gates the output off; reset `0x00` |
//! | `0x503D` | test pattern | bit7 enable, bits1:0 type (0 colour bars, 1 random, 2 colour squares, 3 black), bit6 rolling, bit4 black and white squares |
//! | `0x3808..0x380B` | output size | read/write, default 2592x1944; the PL's raster decides the geometry |
//! | everything else | | plain read/write storage, 64 KiB |
//!
//! The sensor streams when it is powered (`CAM_GPIO` high), `0x3008` bit6 is
//! clear and `0x4202` low nibble is 0. While it does not stream, the PL's
//! video input reads black; while it streams with the test pattern disabled
//! it shows the host's video source (`[media] video_in`).

use crate::i2c::{I2cSlave, PointerSlave, Registers};
use crate::{BoardDevice, DeviceImage, DeviceRegister, PinDrive, readout};

/// SCCB address of the OV5640 (7 bit).
pub const ADDRESS: u8 = 0x3C;
const GPIO_BIT: u8 = 0;
const SCL_BIT: u8 = 2;
const SDA_BIT: u8 = 3;

const SYS_CTRL0: usize = 0x3008;
const CHIP_ID_H: usize = 0x300A;
const CHIP_ID_L: usize = 0x300B;
const STREAM_CTRL: usize = 0x4202;
const TEST_PATTERN: usize = 0x503D;
const OUT_W_H: usize = 0x3808;

/// The eight colour bars, left to right.
pub const BARS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
];

/// The register file and what hangs off it.
#[derive(Clone, Debug)]
struct Sccb {
    regs: Vec<u8>,
    powered: bool,
    reads: u32,
    writes: u32,
    resets: u32,
}

impl Sccb {
    fn new() -> Sccb {
        let mut s = Sccb {
            regs: vec![0; 0x1_0000],
            powered: true,
            reads: 0,
            writes: 0,
            resets: 0,
        };
        s.defaults();
        s
    }

    fn defaults(&mut self) {
        self.regs.fill(0);
        self.regs[CHIP_ID_H] = 0x56;
        self.regs[CHIP_ID_L] = 0x40;
        self.regs[SYS_CTRL0] = 0x42;
        // 2592 x 1944.
        self.regs[OUT_W_H] = 0x0A;
        self.regs[OUT_W_H + 1] = 0x20;
        self.regs[OUT_W_H + 2] = 0x07;
        self.regs[OUT_W_H + 3] = 0x98;
    }

    fn streaming(&self) -> bool {
        self.powered && self.regs[SYS_CTRL0] & 0x40 == 0 && self.regs[STREAM_CTRL] & 0x0F == 0
    }

    fn pattern(&self) -> Option<u8> {
        let v = self.regs[TEST_PATTERN];
        (v & 0x80 != 0).then_some(v)
    }
}

impl Registers for Sccb {
    fn read(&mut self, reg: u32) -> u8 {
        self.reads += 1;
        self.regs[(reg & 0xFFFF) as usize]
    }

    fn write(&mut self, reg: u32, v: u8) {
        self.writes += 1;
        let r = (reg & 0xFFFF) as usize;
        match r {
            CHIP_ID_H | CHIP_ID_L => {}
            SYS_CTRL0 => {
                if v & 0x80 != 0 {
                    self.defaults();
                    self.resets += 1;
                }
                self.regs[SYS_CTRL0] = v & 0x7F;
            }
            _ => self.regs[r] = v,
        }
    }
}

/// A pseudo-random byte from a position, for the random test pattern.
fn noise(frame: u32, x: u32, y: u32, c: u32) -> u8 {
    let mut h = frame
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(x.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE35))
        .wrapping_add(c.wrapping_mul(0x27D4_EB2F));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as u8
}

/// Line `y` of frame `frame` of the sensor's test pattern `ctrl` (the value of
/// `0x503D`), `w` pixels wide in a `h`-line picture, as `3 * w` bytes of RGB.
///
/// * type 0, colour bars: eight vertical bars; with the rolling bit the bars
///   shift one place every frame;
/// * type 1: pseudo-random noise (a pure function of frame, x and y);
/// * type 2, colour squares: an 8 x 8 grid, the colour index is
///   `(column + row) % 8`; with bit 4 black and white;
/// * type 3: black.
pub fn pattern_line(ctrl: u8, frame: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let rolling = ctrl & 0x40 != 0;
    let mut out = Vec::with_capacity((w * 3) as usize);
    for x in 0..w {
        let col = x * 8 / w.max(1);
        let px: [u8; 3] = match ctrl & 3 {
            0 => {
                let i = if rolling {
                    col.wrapping_add(frame)
                } else {
                    col
                };
                BARS[(i % 8) as usize]
            }
            1 => [
                noise(frame, x, y, 0),
                noise(frame, x, y, 1),
                noise(frame, x, y, 2),
            ],
            2 => {
                let row = y * 8 / h.max(1);
                let i = (col + row + if rolling { frame } else { 0 }) % 8;
                if ctrl & 0x10 != 0 {
                    // Black and white squares.
                    if i % 2 == 0 {
                        [255, 255, 255]
                    } else {
                        [0, 0, 0]
                    }
                } else {
                    BARS[i as usize]
                }
            }
            _ => [0, 0, 0],
        };
        out.extend_from_slice(&px);
    }
    out
}

/// The OV5640 behind a Pcam 5C.
#[derive(Clone, Debug)]
pub struct Ov5640 {
    bus: I2cSlave,
    dev: PointerSlave<Sccb>,
    lines: u64,
    frames: u32,
}

impl Default for Ov5640 {
    fn default() -> Ov5640 {
        Ov5640::new()
    }
}

impl Ov5640 {
    /// A powered sensor in its stopped state.
    pub fn new() -> Ov5640 {
        Ov5640 {
            bus: I2cSlave::new(SCL_BIT, SDA_BIT),
            dev: PointerSlave::new(ADDRESS, 2, Sccb::new()),
            lines: 0,
            frames: 0,
        }
    }

    /// True while the sensor outputs frames (powered, not in software power
    /// down, stream not gated).
    pub fn is_streaming(&self) -> bool {
        self.dev.regs.streaming()
    }

    /// The register `addr` as the SCCB interface would return it.
    pub fn register(&self, addr: u16) -> u8 {
        self.dev.regs.regs[usize::from(addr)]
    }

    /// Frames started on the video input while streaming.
    pub fn frames(&self) -> u32 {
        self.frames
    }
}

impl BoardDevice for Ov5640 {
    fn kind(&self) -> &'static str {
        "pcam"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        let powered = levels >> GPIO_BIT & 1 != 0;
        if powered != self.dev.regs.powered {
            self.dev.regs.powered = powered;
            if !powered {
                // Power removed: registers and the bus interface reset.
                self.dev.regs.defaults();
                self.dev.regs.resets += 1;
                self.bus = I2cSlave::new(SCL_BIT, SDA_BIT);
            }
        }
        if powered {
            self.bus.pins(&mut self.dev, levels);
        }
    }

    fn drive(&self) -> PinDrive {
        PinDrive {
            mask: 1 << SDA_BIT,
            levels: u8::from(self.bus.sda_released()) << SDA_BIT,
        }
    }

    fn video_line(&mut self, frame: u32, y: u32, w: u32, h: u32) -> Option<Vec<u8>> {
        if y == 0 && self.dev.regs.streaming() {
            self.frames += 1;
        }
        self.lines += 1;
        if !self.dev.regs.streaming() {
            return Some(vec![0; (w * 3) as usize]);
        }
        self.dev
            .regs
            .pattern()
            .map(|ctrl| pattern_line(ctrl, frame, y, w, h))
    }

    fn image(&self) -> Option<DeviceImage> {
        let r = &self.dev.regs;
        let state = if !r.powered {
            "POWERED OFF"
        } else if r.streaming() {
            "STREAMING"
        } else {
            "STOPPED"
        };
        let pattern = match r.pattern() {
            None => "HOST IMAGE".to_string(),
            Some(v) => format!(
                "PATTERN {}",
                ["BARS", "RANDOM", "SQUARES", "BLACK"][usize::from(v & 3)]
            ),
        };
        Some(readout(&[
            "PCAM OV5640",
            state,
            &pattern,
            &format!("{} FRAMES", self.frames),
        ]))
    }

    fn describe(&self) -> String {
        let r = &self.dev.regs;
        format!(
            "OV5640 (Pcam 5C) SCCB {ADDRESS:#04x}, {}, SYSTEM_CTRL0 {:#04x}, stream {:#04x}, \
             test pattern {:#04x}, {} register read(s), {} write(s), {} reset(s), {} frame(s) \
             streamed; MIPI CSI-2 not simulated",
            if !r.powered {
                "powered off"
            } else if r.streaming() {
                "streaming"
            } else {
                "stopped"
            },
            r.regs[SYS_CTRL0],
            r.regs[STREAM_CTRL],
            r.regs[TEST_PATTERN],
            r.reads,
            r.writes,
            r.resets,
            self.frames,
        )
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        let r = &self.dev.regs;
        let reg = |name: &str, addr: usize| DeviceRegister {
            name: name.to_string(),
            addr: addr as u32,
            value: u32::from(r.regs[addr]),
        };
        vec![
            reg("CHIP_ID_H", CHIP_ID_H),
            reg("CHIP_ID_L", CHIP_ID_L),
            reg("SYSTEM_CTRL0", SYS_CTRL0),
            reg("STREAM_CTRL", STREAM_CTRL),
            reg("TEST_PATTERN", TEST_PATTERN),
            reg("OUT_WIDTH_H", OUT_W_H),
            reg("OUT_WIDTH_L", OUT_W_H + 1),
            reg("OUT_HEIGHT_H", OUT_W_H + 2),
            reg("OUT_HEIGHT_L", OUT_W_H + 3),
            DeviceRegister {
                name: "POINTER".to_string(),
                addr: 0xFFFF,
                value: self.dev.pointer(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::tests::{Master, SCL, SDA};

    fn master() -> Master<PointerSlave<Sccb>> {
        Master::new(PointerSlave::new(ADDRESS, 2, Sccb::new()))
    }

    fn write_reg(m: &mut Master<PointerSlave<Sccb>>, reg: u16, v: u8) -> Vec<bool> {
        m.write(ADDRESS, &[(reg >> 8) as u8, reg as u8, v])
    }

    fn read_reg(m: &mut Master<PointerSlave<Sccb>>, reg: u16, n: usize) -> Vec<u8> {
        m.write(ADDRESS, &[(reg >> 8) as u8, reg as u8]);
        m.start();
        assert!(m.write_byte(ADDRESS << 1 | 1));
        let v = (0..n).map(|i| m.read_byte(i + 1 < n)).collect();
        m.stop();
        v
    }

    #[test]
    fn chip_id_and_sixteen_bit_register_access() {
        let mut m = master();
        assert_eq!(read_reg(&mut m, 0x300A, 2), vec![0x56, 0x40]);
        // The ID is read only.
        write_reg(&mut m, 0x300A, 0x99);
        assert_eq!(read_reg(&mut m, 0x300A, 1), vec![0x56]);
        // A plain register, and a sequential write.
        assert!(write_reg(&mut m, 0x3103, 0x11).iter().all(|a| *a));
        assert_eq!(read_reg(&mut m, 0x3103, 1), vec![0x11]);
        m.write(ADDRESS, &[0x38, 0x00, 1, 2, 3]);
        assert_eq!(read_reg(&mut m, 0x3800, 3), vec![1, 2, 3]);
    }

    #[test]
    fn software_reset_restores_defaults_and_clears_itself() {
        let mut m = master();
        write_reg(&mut m, 0x3103, 0x11);
        write_reg(&mut m, 0x503D, 0x80);
        write_reg(&mut m, 0x3008, 0x82);
        assert_eq!(read_reg(&mut m, 0x3103, 1), vec![0x00]);
        assert_eq!(read_reg(&mut m, 0x503D, 1), vec![0x00]);
        assert_eq!(
            read_reg(&mut m, 0x3008, 1),
            vec![0x02],
            "bit 7 cleared, the rest as written"
        );
        assert_eq!(m.dev.regs.resets, 1);
        assert_eq!(read_reg(&mut m, 0x300A, 1), vec![0x56]);
    }

    #[test]
    fn streaming_is_gated_by_power_down_and_stream_control() {
        let mut m = master();
        let streaming = |m: &Master<PointerSlave<Sccb>>| m.dev.regs.streaming();
        assert!(!streaming(&m), "stopped at power on");
        write_reg(&mut m, 0x3008, 0x02);
        assert!(streaming(&m));
        write_reg(&mut m, 0x4202, 0x0F);
        assert!(!streaming(&m), "stream gated off");
        write_reg(&mut m, 0x4202, 0x00);
        assert!(streaming(&m));
        write_reg(&mut m, 0x3008, 0x42);
        assert!(!streaming(&m), "software power down");
    }

    /// Bit-bangs SCCB register writes into the device's pins (GPIO high).
    struct Bitbang {
        t: u64,
    }

    impl Bitbang {
        fn set(&mut self, d: &mut Ov5640, scl: bool, sda: bool) {
            self.t += 1;
            let lv = 1 | if scl { SCL } else { 0 } | if sda { SDA } else { 0 };
            d.pins(self.t, lv);
        }

        fn byte(&mut self, d: &mut Ov5640, b: u8) {
            for i in (0..8).rev() {
                let bit = b >> i & 1 != 0;
                self.set(d, false, bit);
                self.set(d, true, bit);
                self.set(d, false, bit);
            }
            self.set(d, false, true);
            self.set(d, true, true);
            self.set(d, false, true);
        }

        fn reg(&mut self, d: &mut Ov5640, r: u16, v: u8) {
            self.set(d, true, true);
            self.set(d, true, false);
            self.set(d, false, false);
            self.byte(d, ADDRESS << 1);
            self.byte(d, (r >> 8) as u8);
            self.byte(d, r as u8);
            self.byte(d, v);
            self.set(d, false, false);
            self.set(d, true, false);
            self.set(d, true, true);
        }
    }

    #[test]
    fn the_device_gates_video_lines_on_the_streaming_state() {
        let mut d = Ov5640::new();
        // Stopped: black, whatever the pattern.
        assert_eq!(d.video_line(0, 0, 8, 8), Some(vec![0; 24]));
        // Run the SCCB writes through the pins.
        let mut bus = Bitbang { t: 0 };
        let mut reg = |d: &mut Ov5640, r: u16, v: u8| bus.reg(d, r, v);
        reg(&mut d, 0x503D, 0x80);
        reg(&mut d, 0x3008, 0x02);
        assert!(d.is_streaming());
        let line = d.video_line(0, 0, 16, 8).unwrap();
        assert_eq!(&line[..3], &BARS[0]);
        assert_eq!(&line[15 * 3..], &BARS[7]);
        assert_eq!(d.frames(), 1);
        // Pattern off: the host's image.
        reg(&mut d, 0x503D, 0x00);
        assert_eq!(d.video_line(1, 0, 16, 8), None);
        // Power removed: black, registers back to defaults.
        d.pins(1000, 0);
        assert_eq!(d.video_line(2, 0, 4, 4), Some(vec![0; 12]));
        assert_eq!(d.register(0x503D), 0);
        assert_eq!(d.register(0x3008), 0x42);
        // The bus is dead while unpowered: no ACK drive.
        assert_eq!(d.drive().levels & SDA, SDA);
    }

    #[test]
    fn patterns_follow_the_documented_geometry() {
        // Standard bars: eight equal columns, the same on every row.
        let a = pattern_line(0x80, 0, 0, 64, 48);
        let b = pattern_line(0x80, 0, 47, 64, 48);
        assert_eq!(a, b);
        for (i, bar) in BARS.iter().enumerate() {
            assert_eq!(&a[i * 8 * 3..i * 8 * 3 + 3], bar);
            assert_eq!(&a[(i * 8 + 7) * 3..(i * 8 + 8) * 3], bar);
        }
        // Rolling bars shift one place per frame.
        let r1 = pattern_line(0xC0, 1, 0, 64, 48);
        assert_eq!(&r1[..3], &BARS[1]);
        // Squares: the colour index is column + row.
        let s = pattern_line(0x82, 0, 6 * 3, 64, 48);
        assert_eq!(&s[..3], &BARS[3], "row 3, column 0");
        let bw = pattern_line(0x92, 0, 0, 64, 48);
        assert_eq!(&bw[..3], &[255, 255, 255]);
        assert_eq!(&bw[8 * 3..8 * 3 + 3], &[0, 0, 0]);
        // Black and noise.
        assert!(pattern_line(0x83, 0, 0, 16, 8).iter().all(|&v| v == 0));
        let n0 = pattern_line(0x81, 0, 3, 16, 8);
        assert_eq!(n0, pattern_line(0x81, 0, 3, 16, 8), "deterministic");
        assert_ne!(
            n0,
            pattern_line(0x81, 1, 3, 16, 8),
            "changes with the frame"
        );
        assert!(n0.iter().any(|&v| v != 0));
    }

    #[test]
    fn describe_and_registers_say_what_is_not_simulated() {
        let d = Ov5640::new();
        assert!(d.describe().contains("MIPI CSI-2 not simulated"));
        assert_eq!(d.kind(), "pcam");
        assert!(
            d.registers()
                .iter()
                .any(|r| r.name == "CHIP_ID_L" && r.value == 0x40)
        );
        assert!(d.image().is_some());
        assert!(d.values().is_empty());
    }
}
