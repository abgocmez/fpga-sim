//! Minimal bare-metal runtime for the Zynq-7000 PS under the simulator:
//! startup, panic handler, UART1 output and the SLCR bring-up the FSBL would
//! normally do. `no_std`, no allocator, no interrupts.

#![no_std]

pub mod irq;

use core::arch::global_asm;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};

// Entry: stack, zero .bss, call `main`, park.
global_asm!(
    ".section .text._start,\"ax\"",
    ".global _start",
    "_start:",
    "    cpsid if",
    "    mrc p15, 0, r0, c0, c0, 5",   // MPIDR
    "    ands r0, r0, #3",
    "    bne 3f",                      // core 1 parks; only core 0 runs `main`
    "    ldr sp, =_stack_top",
    "    ldr r0, =__bss_start",
    "    ldr r1, =__bss_end",
    "    mov r2, #0",
    "1:  cmp r0, r1",
    "    strlo r2, [r0], #4",
    "    blo 1b",
    "    bl main",
    "2:  wfi",
    "    b 2b",
    "3:  wfe",
    "    b 3b",
);

const UART1: usize = 0xE000_1000;
const UART_SR: usize = 0x2C;
const UART_FIFO: usize = 0x30;
const SR_TXFULL: u32 = 1 << 4;

const SLCR_UNLOCK: usize = 0xF800_0008;
const SLCR_LOCK: usize = 0xF800_0004;
const SLCR_FPGA_RST_CTRL: usize = 0xF800_0240;
const SLCR_LVL_SHFTR_EN: usize = 0xF800_0900;

/// Writes a 32-bit device register.
#[inline]
pub fn write32(addr: usize, v: u32) {
    // SAFETY: callers pass addresses of device registers or RAM they own; the
    // simulated bus reports an access error for anything else.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Reads a 32-bit device register.
#[inline]
pub fn read32(addr: usize) -> u32 {
    // SAFETY: see `write32`.
    unsafe { read_volatile(addr as *const u32) }
}

/// Enables the PS-PL level shifters and releases the FPGA resets, as the FSBL
/// (`ps7_init`) does on hardware. QEMU runs no FSBL.
pub fn pl_bringup() {
    write32(SLCR_UNLOCK, 0xDF0D);
    write32(SLCR_LVL_SHFTR_EN, 0xF);
    write32(SLCR_FPGA_RST_CTRL, 0);
    write32(SLCR_LOCK, 0x767B);
}

/// Busy-waits roughly `us` microseconds of simulated time.
///
/// Meant for pacing register polls. Under the simulator every register read
/// is a round trip to the PL that costs far more real time than the few
/// nanoseconds of simulated time it takes, so a loop that polls back to back
/// makes simulated time crawl. The length is an estimate: it is calibrated for
/// `-icount shift=1` (2 ns per instruction) and a three-instruction loop.
pub fn delay_us(us: u32) {
    for _ in 0..us.saturating_mul(160) {
        // SAFETY: a no-op; keeps the loop from being optimised away.
        unsafe { core::arch::asm!("nop") }
    }
}

/// Sends one byte on UART1.
pub fn putc(b: u8) {
    while read32(UART1 + UART_SR) & SR_TXFULL != 0 {}
    write32(UART1 + UART_FIFO, u32::from(b));
}

/// Prints on UART1; use through `print!`/`println!`.
pub struct Uart;

impl fmt::Write for Uart {
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

/// `print!` to UART1.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use ::core::fmt::Write as _;
        let _ = write!($crate::Uart, $($arg)*);
    }};
}

/// `println!` to UART1 (CR LF line ends).
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{ $crate::print!($($arg)*); $crate::print!("\n"); }};
}

/// Marks the end of a simulated run: the testbench ends the simulation when
/// the PL sees this write (the prototype's end-of-test convention).
pub fn finish() -> ! {
    println!("COSIM_DONE");
    write32(0x43C0_003C, 0xC051_D0DE);
    loop {
        // SAFETY: parks the core until an interrupt that never comes.
        unsafe { core::arch::asm!("wfi") }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\nPANIC: {info}");
    loop {
        // SAFETY: as in `finish`.
        unsafe { core::arch::asm!("wfi") }
    }
}
