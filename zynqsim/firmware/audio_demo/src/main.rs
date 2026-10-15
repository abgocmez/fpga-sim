//! Audio demo for the Zybo Z7 codec (SSM2603) behind the `audio_ctl` PL block.
//!
//! 1. Brings the codec up over I2C (bit-banged through `audio_ctl`).
//! 2. Plays a 1 kHz tone for a while and reports the frames sent.
//! 3. Switches to loopback: LINE IN is sent back to the headphone output, and
//!    the ADC peak register is read to show that audio arrives.
//!
//! The host sees the tone and the loopback as the codec's DAC output
//! (`audio_capture`, `--audio-out`) and chooses the LINE IN source
//! (`--audio-in`, `[media] audio_in`).
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const AUDIO: usize = 0x43C0_0000;
const CTRL: usize = AUDIO;
const STEP: usize = AUDIO + 0x04;
const AMP: usize = AUDIO + 0x08;
const FRAMES: usize = AUDIO + 0x0C;
const ADC_L: usize = AUDIO + 0x10;
const ADC_PEAK: usize = AUDIO + 0x18;
const I2C: usize = AUDIO + 0x1C;

const CTRL_ENABLE: u32 = 1;
const CTRL_TONE: u32 = 2;
const CTRL_LOOP: u32 = 4;

const CODEC_ADDR: u8 = 0x1A;

/// Tone phase step per 48 kHz frame for `hz`: `hz * 2^32 / 48000`.
const fn phase_step(hz: u32) -> u32 {
    ((hz as u64 * (1u64 << 32)) / 48_000) as u32
}

fn i2c(scl: u32, sda: u32) {
    write32(I2C, scl | (sda << 1));
}

/// Clocks one byte out MSB first; returns true when the slave ACKs.
fn i2c_byte(byte: u8) -> bool {
    for i in (0..8).rev() {
        let bit = u32::from((byte >> i) & 1);
        i2c(0, bit);
        i2c(1, bit);
        i2c(0, bit);
    }
    i2c(0, 1); // release SDA for the ACK slot
    i2c(1, 1);
    let ack = read32(I2C) & 0x100 == 0; // the slave pulls SDA low
    i2c(0, 1);
    ack
}

/// Writes one 9-bit SSM2603 register: `{reg[6:0], value[8]} {value[7:0]}`.
fn codec_write(reg: u8, value: u16) -> bool {
    i2c(1, 1);
    i2c(1, 0); // START
    i2c(0, 0);
    let a = i2c_byte(CODEC_ADDR << 1);
    let b = i2c_byte((reg << 1) | ((value >> 8) as u8 & 1));
    let c = i2c_byte(value as u8);
    i2c(0, 0);
    i2c(1, 0);
    i2c(1, 1); // STOP
    a && b && c
}

/// Waits until the I2S master has sent `n` frames.
fn wait_frames(n: u32) {
    while read32(FRAMES) < n {
        rt::delay_us(20); // pace the polls, see `rt::delay_us`
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("audio demo: SSM2603 over I2C, I2S master in the PL");

    // Power up, unmute, 0 dB headphone, I2S 16 bit, activate.
    let init: [(u8, u16); 8] = [
        (15, 0x000),
        (6, 0x000),
        (2, 0x079),
        (3, 0x079),
        (5, 0x000),
        (7, 0x002),
        (8, 0x000),
        (9, 0x001),
    ];
    let mut acked = 0;
    for (reg, value) in init {
        if codec_write(reg, value) {
            acked += 1;
        }
    }
    println!("codec: {acked}/{} register writes acknowledged", init.len());

    // ---- phase 1: 1 kHz triangle tone ----
    write32(STEP, phase_step(1000));
    write32(AMP, 16000);
    write32(CTRL, CTRL_ENABLE | CTRL_TONE);
    wait_frames(1200); // 25 ms
    println!("TONE 1000 Hz amplitude 16000 frames={}", read32(FRAMES));

    // ---- phase 2: LINE IN -> headphones ----
    write32(CTRL, 0);
    rt::delay_us(5); // the I2S master needs a few MCLK cycles to see ENABLE low
    write32(CTRL, CTRL_ENABLE | CTRL_LOOP);
    write32(ADC_PEAK, 0); // any write clears the peak
    wait_frames(1000); // 21 ms
    let peak = read32(ADC_PEAK);
    let left = read32(ADC_L) as i32;
    println!("LOOPBACK frames={} adc_peak={peak} adc_left={left}", read32(FRAMES));

    write32(CTRL, 0);
    if acked == init.len() {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL codec did not acknowledge");
    }
    rt::finish()
}
