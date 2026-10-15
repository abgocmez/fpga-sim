//! Pcam 5C (OmniVision OV5640) demo: the SCCB control interface through the
//! `i2c_master` PL block and frame capture from the camera's pixel bus into
//! DDR through `video_fb`.
//!
//! The MIPI CSI-2 link of the real module is not simulated; the sensor model
//! on the host gives the PL a parallel pixel bus (the video input of the
//! testbench). What this program shows is the part that is simulated:
//!
//! * `POWER`: with CAM_GPIO low the camera does not answer on SCCB;
//! * `CHIP ID` `0x5640`, the software reset and power-down bits of `0x3008`;
//! * `CAP <label> checksum 0x...`: an FNV-1a checksum of a whole captured
//!   frame (160x120, 32-bit xRGB) while the sensor is stopped, while it
//!   streams colour bars and colour squares, with the test pattern off (the
//!   host's image), and with the stream gated off by `0x4202`.
#![no_std]
#![no_main]

use core::ptr::read_volatile;
use rt::{println, read32, write32};

const I2C: usize = 0x43C0_0000;
const CMD: usize = I2C;
const TXDATA: usize = I2C + 0x04;
const RXDATA: usize = I2C + 0x08;
const STATUS: usize = I2C + 0x0C;
const DIV: usize = I2C + 0x10;
const GPO: usize = I2C + 0x18;

const START: u32 = 1 << 0;
const STOP: u32 = 1 << 1;
const WRITE: u32 = 1 << 2;
const READ: u32 = 1 << 3;
const NACK: u32 = 1 << 4;

const VIDEO: usize = 0x43C1_0000;
const V_CTRL: usize = VIDEO;
const V_CAP_ADDR: usize = VIDEO + 0x08;
const V_STATUS: usize = VIDEO + 0x0C;
const V_CAP_FRAMES: usize = VIDEO + 0x14;
const V_SIZE: usize = VIDEO + 0x18;
const CAP_EN: u32 = 2;
const CAP_DONE: u32 = 1;

const CAP: usize = 0x0200_0000;

/// SCCB address of the OV5640, 7 bit.
const ADDR: u32 = 0x3C;

fn wait_idle() {
    while read32(STATUS) & 1 != 0 {
        rt::delay_us(4);
    }
}

fn command(flags: u32) {
    write32(CMD, flags);
    wait_idle();
}

/// Writes one byte; false if the slave did not ACK it.
fn write_byte(b: u32, flags: u32) -> bool {
    write32(TXDATA, b);
    command(WRITE | flags);
    read32(STATUS) & 2 == 0
}

fn read_byte(flags: u32) -> u32 {
    command(READ | flags);
    read32(RXDATA) & 0xFF
}

/// Writes register `reg` (16-bit address) of the sensor.
fn write_reg(reg: u32, v: u32) -> bool {
    let mut ok = write_byte(ADDR << 1, START);
    ok &= write_byte(reg >> 8, 0);
    ok &= write_byte(reg & 0xFF, 0);
    ok &= write_byte(v, STOP);
    ok
}

/// Reads register `reg`, `out.len()` consecutive registers.
fn read_regs(reg: u32, out: &mut [u32]) {
    write_byte(ADDR << 1, START);
    write_byte(reg >> 8, 0);
    write_byte(reg & 0xFF, STOP);
    write_byte(ADDR << 1 | 1, START);
    let n = out.len();
    for (i, o) in out.iter_mut().enumerate() {
        *o = read_byte(if i + 1 == n { NACK | STOP } else { 0 });
    }
}

fn read_reg(reg: u32) -> u32 {
    let mut b = [0u32; 1];
    read_regs(reg, &mut b);
    b[0]
}

/// FNV-1a over 32-bit words, `0x00RRGGBB` only.
fn checksum(base: usize, words: usize) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for i in 0..words {
        // SAFETY: `base` is a DDR buffer this program owns, `words` long.
        let w = unsafe { read_volatile((base + i * 4) as *const u32) } & 0x00FF_FFFF;
        h = (h ^ w).wrapping_mul(0x0100_0193);
    }
    h
}

/// Captures the next whole frame and prints its checksum.
fn capture(label: &str, words: usize) {
    write32(V_CAP_ADDR, CAP as u32);
    write32(V_STATUS, CAP_DONE); // clear a previous completion
    write32(V_CTRL, CAP_EN);
    let mut spins = 0u32;
    while read32(V_STATUS) & CAP_DONE == 0 {
        spins += 1;
        rt::delay_us(100);
        if spins > 400 {
            println!("RESULT: FAIL capture timed out ({label})");
            rt::finish();
        }
    }
    println!(
        "CAP {label} checksum {:#010x} frames={}",
        checksum(CAP, words),
        read32(V_CAP_FRAMES)
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    let size = read32(V_SIZE);
    let (w, h) = (size & 0xFFFF, size >> 16);
    println!("pcam demo: OV5640 SCCB and a {w}x{h} frame capture");
    let words = (w * h) as usize;
    write32(DIV, 63); // about 400 kHz

    // CAM_GPIO is the camera's power enable (bit 0 of the pin group).
    write32(GPO, 0xFE);
    let off = write_byte(ADDR << 1, START | STOP);
    println!("POWER off ack={off}");
    write32(GPO, 0xFF);
    let on = write_byte(ADDR << 1, START | STOP);
    println!("POWER on ack={on}");

    let mut id = [0u32; 2];
    read_regs(0x300A, &mut id);
    println!("CHIP ID {:#04X}{:02X}", id[0], id[1]);
    println!("SYS_CTRL0 {:#04X}", read_reg(0x3008));

    // Software reset (bit 7 clears itself), keep the sensor stopped.
    write_reg(0x3008, 0x82);
    println!("RESET SYS_CTRL0 {:#04X}", read_reg(0x3008));
    write_reg(0x3008, 0x42);

    // Stopped: the video input reads black even with a pattern selected.
    write_reg(0x503D, 0x80);
    capture("stopped", words);

    // Start streaming: colour bars.
    write_reg(0x3008, 0x02);
    capture("bars", words);

    // Colour squares.
    write_reg(0x503D, 0x82);
    capture("squares", words);

    // Test pattern off: the host's image.
    write_reg(0x503D, 0x00);
    capture("host", words);

    // The stream gate (0x4202) stops the output again.
    write_reg(0x4202, 0x0F);
    capture("gated", words);

    println!("RESULT: ALL PASS");
    rt::finish()
}
