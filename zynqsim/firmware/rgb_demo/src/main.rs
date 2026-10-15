//! Sets the two RGB LEDs through the `rgb_pwm` PL block and reads the duty
//! registers back. The host reconstructs the colours from the PWM pins.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const PWM: usize = 0x43C0_0000;

/// LD5 (R, G, B) then LD6 (R, G, B).
const DUTY: [u32; 6] = [64, 128, 255, 255, 0, 32];

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("rgb demo: LD5 and LD6 through rgb_pwm");
    for (i, d) in DUTY.iter().enumerate() {
        write32(PWM + 4 * i, *d);
    }
    let mut ok = true;
    for (i, d) in DUTY.iter().enumerate() {
        let back = read32(PWM + 4 * i);
        if back != *d {
            println!("duty {i}: wrote {d}, read {back}");
            ok = false;
        }
    }
    // Let the PWM run for a while so the host can average it.
    rt::delay_us(600);
    println!("{}", if ok { "RESULT: ALL PASS" } else { "RESULT: FAIL" });
    rt::finish()
}
