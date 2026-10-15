//! Text on the Digilent Pmod OLED (SSD1306, 128x32) through the `oled_spi`
//! PL block: power-up sequence, display setup, four lines of text.
//!
//! The 5x7 font is the very file the host-side tests use, so the picture the
//! simulator shows can be compared with what these bytes mean.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

#[allow(dead_code)]
mod font {
    include!("../../../crates/devices/src/font5x7.rs");
}

const SPI: usize = 0x43C0_0000;
const DATA: usize = SPI;
const STATUS: usize = SPI + 0x04;
const CTRL: usize = SPI + 0x08;
const DIV: usize = SPI + 0x0C;

const CS: u32 = 1 << 0; // 1 = deselected
const DC: u32 = 1 << 1; // 1 = data
const RES_N: u32 = 1 << 2;
const VBATC: u32 = 1 << 3;
const VDDC: u32 = 1 << 4;

const LINES: [&str; 4] = ["ZYNQSIM OLED", "HELLO FROM RUST", "PS PL 100 MHZ", "RESULT: PASS"];

fn send(b: u8) {
    while read32(STATUS) & 1 != 0 {}
    write32(DATA, u32::from(b));
}

fn wait_idle() {
    while read32(STATUS) & 1 != 0 {}
}

fn command(supply: u32, bytes: &[u8]) {
    write32(CTRL, supply | RES_N); // CS low, D/C low
    for &b in bytes {
        send(b);
    }
    wait_idle();
    write32(CTRL, supply | RES_N | CS);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("oled demo: SSD1306 128x32 on Pmod JA through oled_spi");
    write32(DIV, 4);

    // Power up, pulse the reset.
    write32(CTRL, VDDC | RES_N | CS);
    rt::delay_us(20);
    write32(CTRL, VDDC | CS); // RES low
    rt::delay_us(20);
    write32(CTRL, VDDC | RES_N | CS);
    rt::delay_us(20);

    // Display setup, then panel supply, then on.
    command(VDDC, &[0xAE, 0x8D, 0x14, 0xD9, 0xF1]);
    write32(CTRL, VDDC | VBATC | RES_N | CS);
    rt::delay_us(20);
    command(
        VDDC | VBATC,
        &[0x20, 0x00, 0x21, 0x00, 0x7F, 0x22, 0x00, 0x03, 0xA1, 0xC8, 0xAF],
    );

    // Four lines of text, 6 columns per character, one page per line.
    let mut fb = [0u8; 4 * 128];
    for (page, line) in LINES.iter().enumerate() {
        for (i, c) in line.bytes().enumerate() {
            let g = font::glyph(c);
            let col = i * 6;
            fb[page * 128 + col..page * 128 + col + 5].copy_from_slice(&g);
        }
    }
    // The display is mounted rotated (segment remap + COM flip above), so
    // send the frame mirrored in both axes to read upright.
    let mut out = [0u8; 4 * 128];
    for page in 0..4 {
        for col in 0..128 {
            let b = fb[(3 - page) * 128 + (127 - col)];
            out[page * 128 + col] = b.reverse_bits();
        }
    }
    write32(CTRL, VDDC | VBATC | RES_N | DC); // CS low, data
    for &b in &out {
        send(b);
    }
    wait_idle();
    write32(CTRL, VDDC | VBATC | RES_N | CS);

    println!("sent {} data bytes", out.len());
    println!("RESULT: ALL PASS");
    rt::finish()
}
