//! Reads the Digilent Pmod TMP2 (ADT7420, I2C 0x4B) on Pmod JA through the
//! `i2c_master` PL block and prints what it reads.
//!
//! The host decides the temperature (`[device_values.pmod_ja] temperature`,
//! `--device-set JA.temperature=...`, `pmod_set`), so the lines printed here
//! are what the simulated sensor returned over the simulated bus:
//!
//! * `ID 0xCB`: the ID register;
//! * `BUS div=N id=0x.. raw=0x....`: the same two reads at three bus speeds;
//! * `T13 raw=0x.... mC=N`: 13-bit mode (0.0625 degC per LSB);
//! * `T16 raw=0x.... mC=N`: 16-bit mode (1/128 degC), repeated whenever the
//!   temperature changes.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const I2C: usize = 0x43C0_0000;
const CMD: usize = I2C;
const TXDATA: usize = I2C + 0x04;
const RXDATA: usize = I2C + 0x08;
const STATUS: usize = I2C + 0x0C;
const DIV: usize = I2C + 0x10;

const START: u32 = 1 << 0;
const STOP: u32 = 1 << 1;
const WRITE: u32 = 1 << 2;
const READ: u32 = 1 << 3;
const NACK: u32 = 1 << 4;

const ADDR: u32 = 0x4B;

const REG_TEMP: u32 = 0x00;
const REG_CONFIG: u32 = 0x03;
const REG_ID: u32 = 0x0B;

fn wait_idle() {
    // One poll costs ~100 us of wall time: pace them.
    while read32(STATUS) & 1 != 0 {
        rt::delay_us(8);
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

/// START, address+W, pointer, repeated START, address+R, `n` bytes, STOP.
fn read_regs(reg: u32, out: &mut [u32]) -> bool {
    let mut ok = write_byte(ADDR << 1, START);
    ok &= write_byte(reg, 0);
    ok &= write_byte(ADDR << 1 | 1, START);
    let n = out.len();
    for (i, o) in out.iter_mut().enumerate() {
        *o = read_byte(if i + 1 == n { NACK | STOP } else { 0 });
    }
    ok
}

fn write_reg(reg: u32, v: u32) -> bool {
    let mut ok = write_byte(ADDR << 1, START);
    ok &= write_byte(reg, 0);
    ok &= write_byte(v, STOP);
    ok
}

fn temp_raw() -> u32 {
    let mut b = [0u32; 2];
    read_regs(REG_TEMP, &mut b);
    b[0] << 8 | b[1]
}

fn id() -> u32 {
    let mut b = [0u32; 1];
    read_regs(REG_ID, &mut b);
    b[0]
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("tmp2 demo: ADT7420 on Pmod JA through i2c_master");

    println!("ID {:#04X}", id());

    // The same transaction at 100 kHz, 1 MHz and 12.5 MHz bus clocks: the
    // board answers in the instant of every pin change, whatever the speed.
    for div in [250u32, 25, 2] {
        write32(DIV, div);
        let i = id();
        let t = temp_raw();
        println!("BUS div={div} id={i:#04X} raw={t:#06X}");
    }
    write32(DIV, 250);

    // 13-bit mode (power-on default): 0.0625 degC per LSB, flags in bits 2..0.
    let raw = temp_raw();
    let mc = i32::from(raw as u16 as i16 >> 3) * 625 / 10;
    println!("T13 raw={raw:#06X} mC={mc}");

    // 16-bit mode: 1/128 degC per LSB.
    write_reg(REG_CONFIG, 0x80);
    let mut last = u32::MAX;
    loop {
        let raw = temp_raw();
        if raw != last {
            last = raw;
            let mc = i32::from(raw as u16 as i16) * 1000 / 128;
            println!("T16 raw={raw:#06X} mC={mc}");
        }
        rt::delay_us(200);
    }
}
