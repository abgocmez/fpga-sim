//! Smallest possible firmware: prints on UART1 from Rust.
#![no_std]
#![no_main]

use rt::println;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("hello from Rust on the simulated Cortex-A9");
    let mut sum = 0u32;
    for i in 1..=100u32 {
        sum += i;
    }
    println!("sum(1..=100) = {sum}");
    println!("RESULT: ALL PASS");
    loop {}
}
