//! HDMI demo: scan a frame buffer out to the HDMI TX connector and capture a
//! frame arriving on HDMI RX, through the `video_fb` PL block.
//!
//! The firmware draws a test card in DDR, scans it out, arms a capture of the
//! incoming picture, and prints a checksum of both buffers so a host can verify
//! them. The host sees the TX picture (`video_frame`, `--video-out`) and chooses
//! the RX picture (`--video-in`, `[media] video_in`).
#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use rt::{println, read32, write32};

const VIDEO: usize = 0x43C1_0000;
const CTRL: usize = VIDEO;
const FB_ADDR: usize = VIDEO + 0x04;
const CAP_ADDR: usize = VIDEO + 0x08;
const STATUS: usize = VIDEO + 0x0C;
const SCAN_FRAMES: usize = VIDEO + 0x10;
const CAP_FRAMES: usize = VIDEO + 0x14;
const SIZE: usize = VIDEO + 0x18;

const SCAN_EN: u32 = 1;
const CAP_EN: u32 = 2;
const CAP_DONE: u32 = 1;

const FB: usize = 0x0100_0000;
const CAP: usize = 0x0200_0000;

/// The test card pixel at (x, y), 0x00RRGGBB. The host computes the same.
fn card(x: u32, y: u32, w: u32, h: u32) -> u32 {
    let r = x * 255 / w;
    let g = y * 255 / h;
    let b = (x ^ y) & 0xFF;
    (r << 16) | (g << 8) | b
}

/// FNV-1a over 32-bit words (one xor and multiply per word).
fn checksum(base: usize, words: usize) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for i in 0..words {
        // SAFETY: `base` is a DDR buffer this program owns, `words` long.
        let w = unsafe { read_volatile((base + i * 4) as *const u32) } & 0x00FF_FFFF;
        h = (h ^ w).wrapping_mul(0x0100_0193);
    }
    h
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    let size = read32(SIZE);
    let (w, h) = (size & 0xFFFF, size >> 16);
    println!("hdmi demo: {w}x{h} frame buffer scan-out and capture");
    if w == 0 || h == 0 {
        println!("RESULT: FAIL no video block");
        rt::finish();
    }
    let words = (w * h) as usize;

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            // SAFETY: the frame buffer is DDR this program owns.
            unsafe { write_volatile((FB + i * 4) as *mut u32, card(x, y, w, h)) }
        }
    }
    println!("FB 0x{FB:08x} checksum 0x{:08x}", checksum(FB, words));

    // Scan out until two whole frames have gone by.
    write32(FB_ADDR, FB as u32);
    write32(CTRL, SCAN_EN);
    while read32(SCAN_FRAMES) < 2 {
        rt::delay_us(50);
    }
    println!("SCANOUT frames={}", read32(SCAN_FRAMES));

    // Capture the next whole frame from HDMI RX while scanning continues.
    write32(CAP_ADDR, CAP as u32);
    write32(CTRL, SCAN_EN | CAP_EN);
    let mut spins = 0u32;
    while read32(STATUS) & CAP_DONE == 0 {
        spins += 1;
        rt::delay_us(50);
        if spins > 1000 {
            println!("RESULT: FAIL capture timed out");
            rt::finish();
        }
    }
    println!(
        "CAPTURE frames={} checksum 0x{:08x}",
        read32(CAP_FRAMES),
        checksum(CAP, words)
    );

    write32(CTRL, 0);
    println!("RESULT: ALL PASS");
    rt::finish()
}
