//! Thumb-2 firmware for the native PS engine (`examples/native-thumb`). Built for
//! `thumbv7a-none-eabi`, so the code the compiler emits is Thumb and Thumb-2 (16-bit and 32-bit
//! encodings, IT blocks, table branches, PUSH and POP, long multiplies, bitfields). It prints a
//! line per check on UART1, like `firmware/hello`, and ends with `RESULT: ALL PASS` when every
//! check holds. The expected values are fixed constants; nothing here depends on timing.
#![no_std]
#![no_main]

use core::arch::global_asm;
use core::fmt::{self, Write};
use core::ptr::{read_volatile, write_volatile};

// Entry: stack, zero .bss, call `main`, park. Written in Thumb-2 (this target's default).
global_asm!(
    ".section .text._start,\"ax\",%progbits",
    ".global _start",
    ".type _start, %function",
    "_start:",
    "    cpsid if",
    "    ldr sp, =_stack_top",
    "    ldr r0, =__bss_start",
    "    ldr r1, =__bss_end",
    "    movs r2, #0",
    "1:  cmp r0, r1",
    "    bhs 2f",
    "    str r2, [r0], #4",
    "    b 1b",
    "2:  bl main",
    "3:  wfi",
    "    b 3b",
);

const UART1: usize = 0xE000_1000;
const UART_SR: usize = 0x2C;
const UART_FIFO: usize = 0x30;
const SR_TXFULL: u32 = 1 << 4;

/// UART1 as a `fmt::Write` sink with CR LF line ends.
struct Uart;

fn putc(b: u8) {
    // SAFETY: UART1 is a device register window of the simulated PS; the bus refuses anything
    // outside it.
    unsafe {
        while read_volatile((UART1 + UART_SR) as *const u32) & SR_TXFULL != 0 {}
        write_volatile((UART1 + UART_FIFO) as *mut u32, u32::from(b));
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                putc(b'\r');
            }
            putc(b);
        }
        Ok(())
    }
}

macro_rules! say {
    ($($arg:tt)*) => {{ let _ = write!(Uart, $($arg)*); }};
}

/// Recursive, so the calls push and pop LR and the saved registers.
#[inline(never)]
fn factorial(n: u32) -> u32 {
    if n <= 1 { 1 } else { n.wrapping_mul(factorial(n - 1)) }
}

/// A small dense match: the compiler builds a table branch for it.
#[inline(never)]
fn classify(x: u32) -> u32 {
    match x {
        0 => 10,
        1 => 21,
        2 => 32,
        3 => 43,
        4 => 54,
        5 => 65,
        _ => 0,
    }
}

#[inline(never)]
fn fib(n: u32) -> u32 {
    let (mut a, mut b) = (0u32, 1u32);
    for _ in 0..n {
        let t = a.wrapping_add(b);
        a = b;
        b = t;
    }
    a
}

#[inline(never)]
fn diff(a: u32, b: u32) -> u32 {
    // Compiles to an IT block with conditional subtracts.
    if a > b { a - b } else { b - a }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    say!("hello from Thumb on the simulated Cortex-A9\n");
    let mut failures = 0u32;
    let mut check = |name: &str, got: u32, want: u32| {
        if got == want {
            say!("ok: {}\n", name);
        } else {
            failures += 1;
            say!("FAIL: {} got {} want {}\n", name, got, want);
        }
    };
    let sum: u32 = (1..=100u32).sum();
    check("sum 1..=100", sum, 5050);
    check("fib(30)", fib(30), 832_040);
    check("factorial(10)", factorial(10), 3_628_800);
    let wide = u64::from(0xDEAD_BEEFu32) * u64::from(0x1234_5678u32);
    check("umull high", (wide >> 32) as u32, 265_666_030);
    check("umull low", wide as u32, 1_445_054_984);
    check("popcount", 0xF0F0_1234u32.count_ones(), 13);
    check("clz", 0x00F0_0000u32.leading_zeros(), 8);
    check("rbit", 0x1234_5678u32.reverse_bits(), 510_274_632);
    check("rev", 0x1234_5678u32.swap_bytes(), 2_018_915_346);
    check("ubfx", (0xABCD_1234u32 >> 4) & 0xFF, 0x23);
    check("sign extend i8", (-5i8) as i32 as u32, 0xFFFF_FFFB);
    check("table branch", classify(3), 43);
    check("it-block diff", diff(3, 10) + diff(10, 3), 14);
    if failures == 0 {
        say!("RESULT: ALL PASS\n");
    } else {
        say!("RESULT: {} FAILED\n", failures);
    }
    loop {
        // SAFETY: parks the core until an interrupt that never comes.
        unsafe { core::arch::asm!("wfi") }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    say!("\nPANIC\n");
    loop {
        // SAFETY: as in `main`.
        unsafe { core::arch::asm!("wfi") }
    }
}
