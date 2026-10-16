//! A PL that mixes the user's RTL (the math IP, in XSim, at 0x43C00000) with a Xilinx IP model
//! (the AXI GPIO of `fixtures/xsa/zs_math_gpio.xsa`, native, at 0x41200000): the same firmware
//! talks to both and the board LEDs show the GPIO's value.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const MATH: usize = 0x43C0_0000;
const GPIO: usize = 0x4120_0000;
const GPIO_DATA: usize = GPIO;
const GPIO_TRI: usize = GPIO + 0x04;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vendor mix: math RTL + AXI GPIO model");

    write32(MATH, 0x15);
    let doubled = read32(MATH + 4);
    println!("math: 0x15 * 2 = {doubled:#x}");

    write32(GPIO_DATA, 0b1001);
    let leds = read32(GPIO_DATA);
    println!("gpio: tri={:#x} data={leds:#x}", read32(GPIO_TRI));

    if doubled == 0x2A && leds == 0b1001 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    rt::finish()
}
