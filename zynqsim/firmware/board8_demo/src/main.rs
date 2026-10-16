//! Eight-bit board I/O demo for the ZedBoard definition: drives all 8 LEDs through the
//! 8-bit `gpio_ctrl` of `fixtures/rtl/board8`, then mirrors the 8 slide switches and the
//! 7 push buttons onto them.
//!
//! 1. LED = 0xA5 (LEDs 0, 2, 5 and 7 on).
//! 2. Waits for any switch, prints `SW=0x..` and shows it on the LEDs.
//! 3. Waits for any button, prints `BTN=0x..` and shows it on the LEDs.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const GPIO: usize = 0x43C0_0000;
const LED: usize = GPIO;
const SW: usize = GPIO + 0x04;
const BTN: usize = GPIO + 0x08;

/// Polls `reg` until it is non-zero (at most about 30 ms of simulated time).
fn wait_nonzero(reg: usize) -> Option<u32> {
    for _ in 0..10_000 {
        let v = read32(reg);
        if v != 0 {
            return Some(v);
        }
        rt::delay_us(10);
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("board8 demo: 8 LEDs, 8 switches, 7 buttons");

    write32(LED, 0xA5);
    println!("LED=0x{:02x}", read32(LED));

    let mut ok = true;
    match wait_nonzero(SW) {
        Some(sw) => {
            println!("SW=0x{sw:02x}");
            write32(LED, sw);
        }
        None => {
            println!("no switch within 30 ms");
            ok = false;
        }
    }
    match wait_nonzero(BTN) {
        Some(btn) => {
            println!("BTN=0x{btn:02x}");
            write32(LED, btn);
        }
        None => {
            println!("no button within 30 ms");
            ok = false;
        }
    }
    if ok {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    rt::finish()
}
