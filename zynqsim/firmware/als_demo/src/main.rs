//! Reads the Digilent Pmod ALS (TI ADC081S021) on Pmod JA through the
//! `spi_master` PL block (SPI mode 0, 16-bit frames) and prints what it reads.
//!
//! The host decides the light level (`[device_values.pmod_ja] light`,
//! `--device-set JA.light=...`, `pmod_set`):
//!
//! * `SPI div=N frame=0x....`: one frame at three bus speeds;
//! * `LIGHT frame=0x.... code=N`: the 8-bit code (frame >> 4), repeated
//!   whenever it changes.
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
/// SPI mode 0: CPOL 0, CPHA 0.
const MODE: u32 = 0;

fn xfer(b: u32) -> u32 {
    write32(DATA, b);
    while read32(STATUS) & 1 != 0 {
        rt::delay_us(2);
    }
    read32(RXDATA) & 0xFF
}

/// One 16-bit conversion frame.
fn frame() -> u32 {
    write32(CTRL, MODE); // CS low
    let hi = xfer(0);
    let lo = xfer(0);
    write32(CTRL, MODE | CS_N);
    hi << 8 | lo
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("als demo: ADC081S021 on Pmod JA through spi_master");
    write32(CTRL, MODE | CS_N);

    // The same frame at 1 MHz, 10 MHz and 50 MHz bus clocks.
    for div in [50u32, 5, 1] {
        write32(DIV, div);
        let f = frame();
        println!("SPI div={div} frame={f:#06X}");
    }
    write32(DIV, 50);

    let mut last = u32::MAX;
    loop {
        let f = frame();
        if f != last {
            last = f;
            println!("LIGHT frame={f:#06X} code={}", f >> 4 & 0xFF);
        }
        rt::delay_us(100);
    }
}
