//! Rate probe of the Cortex-A9 global timer (0xF8F00200) and private timer
//! (0xF8F00600): each prints one `DONE_*` line once its counter has advanced
//! by 1,000,000 ticks. The host (`crates/cli/tests/a9_timer_rate.rs`) reads
//! the simulated time at which that line appears, which gives the tick rate:
//! 333.33 MHz (the CPU clock of the reference board / 2) needs 3 ms, the
//! 100 MHz of the unmodified QEMU tree needs 10 ms.
//!
//! The firmware does no clock setup itself: the rate comes from the machine
//! (see `[ps] cpu_clock`, ADR-029). Polls are plain reads; the loops are
//! short in simulated time.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const GT_LO: usize = 0xF8F0_0200;
const PT_LOAD: usize = 0xF8F0_0600;
const PT_COUNTER: usize = 0xF8F0_0604;
const PT_CONTROL: usize = 0xF8F0_0608;
const TICKS: u32 = 1_000_000;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    let g0 = read32(GT_LO);
    while read32(GT_LO).wrapping_sub(g0) < TICKS {}
    println!(
        "DONE_GT ticks={TICKS} start={g0:08x} end={:08x}",
        read32(GT_LO)
    );

    // The private timer counts down from LOAD, with prescaler 0 and no IRQ.
    write32(PT_LOAD, 0xFFFF_FFFF);
    write32(PT_CONTROL, 0x1);
    let p0 = read32(PT_COUNTER);
    while p0.wrapping_sub(read32(PT_COUNTER)) < TICKS {}
    println!(
        "DONE_PT ticks={TICKS} start={p0:08x} end={:08x}",
        read32(PT_COUNTER)
    );

    loop {
        // SAFETY: parks the core; nothing wakes it.
        unsafe { core::arch::asm!("wfi") }
    }
}
