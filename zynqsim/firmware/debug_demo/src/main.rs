//! Source-level debugging demo. Built with line tables (`debug =
//! "line-tables-only"` for this package in `firmware/Cargo.toml`), so the
//! simulator's debugger can map a stop back to a line of this file.
//!
//! The `MARK:` comments name lines that the end-to-end tests set breakpoints
//! on and compare the reported `file:line` with; move the code freely, the
//! tests find the marks by text.
#![no_std]
#![no_main]

use core::hint::black_box;
use rt::println;

/// Number of 3n+1 steps until `n` reaches 1.
#[inline(never)]
fn collatz_steps(mut n: u32) -> u32 {
    let mut steps = 0;
    while n != 1 {
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 }; // MARK:collatz-step
        steps += 1;
    }
    steps
}

/// Sum of the bytes, wrapping.
#[inline(never)]
fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for &b in data {
        sum = sum.wrapping_add(u32::from(b));
    }
    sum
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("debug demo");
    let steps = collatz_steps(black_box(27)); // MARK:call-collatz
    println!("collatz(27) = {steps} steps");
    let sum = checksum(black_box(b"ZynqSim source view")); // MARK:call-checksum
    println!("checksum = {sum}");
    if steps == 111 && sum == 1895 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    loop {}
}
