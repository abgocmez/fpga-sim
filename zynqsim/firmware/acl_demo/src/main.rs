//! Reads the Digilent Pmod ACL (ADXL345) on Pmod JA through the `spi_master`
//! PL block (SPI mode 3, 4-wire) and prints the acceleration.
//!
//! The host decides the acceleration (`[device_values.pmod_ja] accel_x`, ...,
//! `--device-set JA.accel_z=1.0`, `pmod_set`):
//!
//! * `ID 0xE5`: DEVID;
//! * `CFG power=0x.. format=0x.. rate=0x..`: the registers written and read back;
//! * `ACC x=N y=N z=N mg`: milli-g from the full-resolution output
//!   (256 LSB/g), repeated whenever a sample changes.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const SPI: usize = 0x43C0_0000;
const DATA: usize = SPI;
const STATUS: usize = SPI + 0x04;
const CTRL: usize = SPI + 0x08;
const DIV: usize = SPI + 0x0C;
const RXDATA: usize = SPI + 0x10;

const CS_N: u32 = 1 << 2;
/// SPI mode 3: CPOL 1, CPHA 1.
const MODE: u32 = 0b011;

const READ: u32 = 0x80;
const MULTI: u32 = 0x40;

const REG_DEVID: u32 = 0x00;
const REG_BW_RATE: u32 = 0x2C;
const REG_POWER_CTL: u32 = 0x2D;
const REG_DATA_FORMAT: u32 = 0x31;
const REG_DATAX0: u32 = 0x32;

fn xfer(b: u32) -> u32 {
    write32(DATA, b);
    while read32(STATUS) & 1 != 0 {
        rt::delay_us(2);
    }
    read32(RXDATA) & 0xFF
}

fn read_reg(reg: u32) -> u32 {
    write32(CTRL, MODE);
    xfer(READ | reg);
    let v = xfer(0);
    write32(CTRL, MODE | CS_N);
    v
}

fn write_reg(reg: u32, v: u32) {
    write32(CTRL, MODE);
    xfer(reg);
    xfer(v);
    write32(CTRL, MODE | CS_N);
}

/// The six data registers in one multi-byte read, as three signed axes.
fn sample() -> [i32; 3] {
    write32(CTRL, MODE);
    xfer(READ | MULTI | REG_DATAX0);
    let mut b = [0u32; 6];
    for v in &mut b {
        *v = xfer(0);
    }
    write32(CTRL, MODE | CS_N);
    let axis = |i: usize| i32::from((b[i * 2] | b[i * 2 + 1] << 8) as u16 as i16);
    [axis(0), axis(1), axis(2)]
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("acl demo: ADXL345 on Pmod JA through spi_master");
    write32(CTRL, MODE | CS_N);
    write32(DIV, 25); // 2 MHz

    println!("ID {:#04X}", read_reg(REG_DEVID));

    // Full resolution (4 mg/LSB at every range), measurement on.
    write_reg(REG_DATA_FORMAT, 0x08);
    write_reg(REG_POWER_CTL, 0x08);
    println!(
        "CFG power={:#04X} format={:#04X} rate={:#04X}",
        read_reg(REG_POWER_CTL),
        read_reg(REG_DATA_FORMAT),
        read_reg(REG_BW_RATE)
    );

    let mut last = [i32::MAX; 3];
    loop {
        let s = sample();
        if s != last {
            last = s;
            // 256 LSB per g: milli-g = counts * 1000 / 256.
            println!(
                "ACC x={} y={} z={} mg",
                s[0] * 1000 / 256,
                s[1] * 1000 / 256,
                s[2] * 1000 / 256
            );
        }
        rt::delay_us(100);
    }
}
