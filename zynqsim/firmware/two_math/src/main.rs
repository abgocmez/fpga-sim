//! Tests two user AXI4-Lite math IP instances (u_math0 at 0x43C00000, u_math1 at 0x43C10000).
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const MATH0: usize = 0x43C0_0000;
const MATH1: usize = 0x43C1_0000;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("two user math RTL slaves demo");

    write32(MATH0, 21);
    let d0 = read32(MATH0 + 4);
    println!("math0: 21 * 2 = {d0}");

    write32(MATH1, 50);
    let d1 = read32(MATH1 + 4);
    println!("math1: 50 * 2 = {d1}");

    if d0 == 42 && d1 == 100 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    rt::finish()
}

