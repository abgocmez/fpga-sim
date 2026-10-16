//! Two simple Pmod modules on the `pmod_simple` PL block.
//!
//! * A Digilent Pmod 8LD on JA: the firmware writes a pattern to the LEDS
//!   register, later a second one, and the host sees both on the eight pins.
//! * A Pmod BTN on JB: the host sets its buttons and switches
//!   (`--device-set JB.btn0=1`, `pmod_set`); the firmware samples the PINS
//!   register and prints every change as `BTN 0x..`.
//!
//! Lines on UART1: `LEDS 0x..` when the firmware writes a pattern, `BTN 0x..`
//! when the sampled value changes, then `RESULT: ALL PASS`.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

/// The PL block's AXI-Lite base address.
const BLOCK: usize = 0x43C0_0000;
/// 0x00 LEDS (RW, the 8LD on JA), 0x04 PINS (RO, the BTN on JB).
const LEDS: usize = BLOCK;
const PINS: usize = BLOCK + 0x04;

const FIRST: u32 = 0xA5;
const LAST: u32 = 0x3C;
/// Samples of the buttons: about 100 us apart.
const SAMPLES: u32 = 80;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("pmod simple demo: 8LD on JA, BTN on JB");

    write32(LEDS, FIRST);
    println!("LEDS 0x{:02X}", FIRST);

    let mut last = u32::MAX;
    for _ in 0..SAMPLES {
        let v = read32(PINS) & 0xFF;
        if v != last {
            println!("BTN 0x{:02X}", v);
            last = v;
        }
        // One sample per simulated 100 us; polls are cheap in simulated time.
        rt::delay_us(100);
    }

    write32(LEDS, LAST);
    println!("LEDS 0x{:02X}", LAST);
    println!("RESULT: ALL PASS");
    rt::finish()
}
