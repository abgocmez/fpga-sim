//! AXI Timer modes (`examples/timer-modes`): capture mode driven by a board button, and cascade
//! mode (64 bit) across the 32-bit boundary and as a periodic 64-bit counter.
//!
//! The one AXI Timer (`axi_timer_0`, 0x42800000) has its interrupt on IRQ_F2P[0] (GIC 61) and its
//! `CAPTURETRIG0` on `board.btn0`.
//!
//! 1. Capture: timer 0 counts from zero in capture mode. The host presses button 0 at 3 ms
//!    and releases it at 4 ms (`--at 3ms=btn0:down --at 4ms=btn0:up`). The rising edge copies the
//!    counter into `TLR0` and raises the interrupt. The release must not capture again.
//! 2. Cascade carry: `TLR1:TLR0 = 0x0_FFFF_FFF0` counts past 2^32; the high word `TCR1` must
//!    become 1 and the low word restart near 0, with no interrupt (a carry is not a rollover).
//! 3. Cascade periodic: `TLR1:TLR0 = 0xFFFF_FFFF:FFFF_F000` with auto reload is a 4096-clock
//!    64-bit period; three interrupts must arrive and `TCR1` must stay at all ones.
//!
//! Every wait is bounded and paced with `rt::delay_us`. The last line is `RESULT: ALL PASS` or
//! `RESULT: FAIL`.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const TIMER: usize = 0x4280_0000;

// AXI Timer (PG079).
const TCSR0: usize = 0x00;
const TLR0: usize = 0x04;
const TCR0: usize = 0x08;
const TLR1: usize = 0x14;
const TCR1: usize = 0x18;
const MDT: u32 = 1 << 0;
const ARHT: u32 = 1 << 4;
const LOAD: u32 = 1 << 5;
const ENIT: u32 = 1 << 6;
const ENT: u32 = 1 << 7;
const TINT: u32 = 1 << 8;
const CAPT: u32 = 1 << 3;
const CASC: u32 = 1 << 11;

const IRQ_TIMER: usize = 61; // IRQ_F2P[0]

/// Interrupts taken from the timer (captures and 64-bit rollovers).
static EVENTS: AtomicU32 = AtomicU32::new(0);

fn timer_isr() {
    let c = read32(TIMER + TCSR0);
    if c & TINT != 0 {
        // Write 1 to TINT to clear it; the rest of the register is written back unchanged.
        write32(TIMER + TCSR0, c);
        EVENTS.fetch_add(1, Ordering::SeqCst);
    }
}

fn events() -> u32 {
    EVENTS.load(Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("timer-modes demo");
    irq::install();
    irq::set_handler(IRQ_TIMER, timer_isr);
    irq::enable_spi(IRQ_TIMER);
    irq::enable();

    // 1. Capture on the rising edge of board button 0.
    write32(TIMER + TLR0, 0);
    write32(TIMER + TCSR0, LOAD);
    write32(TIMER + TCSR0, MDT | CAPT | ENT | ENIT);
    let mut waited = 0;
    while events() == 0 && waited < 5000 {
        rt::delay_us(20);
        waited += 1;
    }
    let captured = read32(TIMER + TLR0);
    let now = read32(TIMER + TCR0);
    let first = events();
    // The release of the tap is not a capture: wait, then count the events again.
    rt::delay_us(1000);
    let after = events();
    // Stop the timer and clear TINT (write 1).
    write32(TIMER + TCSR0, TINT);
    let capture_ok = first == 1 && after == 1 && captured > 0 && captured < now;
    println!(
        "capture tlr={captured:#x} tcr={now:#x} events={after} waited={waited} capture_ok={}",
        u32::from(capture_ok)
    );

    // 2. Cascade carry across the 32-bit boundary (no interrupt: not a 64-bit rollover).
    write32(TIMER + TLR1, 0);
    write32(TIMER + TLR0, 0xFFFF_FFF0);
    write32(TIMER + TCSR0, LOAD | CASC);
    write32(TIMER + TCSR0, ENT | CASC);
    rt::delay_us(5);
    let hi = read32(TIMER + TCR1);
    let lo = read32(TIMER + TCR0);
    let carry_ok = hi == 1 && lo < 0x1_0000 && read32(TIMER + TCSR0) & TINT == 0;
    println!(
        "cascade carry tcr1={hi:#x} tcr0_small={} carry_ok={}",
        u32::from(lo < 0x1_0000),
        u32::from(carry_ok)
    );
    write32(TIMER + TCSR0, 0);

    // 3. Cascade periodic: a 64-bit counter with auto reload, one interrupt per 4096 clocks.
    write32(TIMER + TLR1, 0xFFFF_FFFF);
    write32(TIMER + TLR0, 0xFFFF_F000);
    write32(TIMER + TCSR0, LOAD | CASC);
    EVENTS.store(0, Ordering::SeqCst);
    write32(TIMER + TCSR0, ENT | CASC | ARHT | ENIT);
    waited = 0;
    while events() < 3 && waited < 2000 {
        rt::delay_us(10);
        waited += 1;
    }
    let periodic = events();
    let top = read32(TIMER + TCR1);
    write32(TIMER + TCSR0, 0);
    write32(TIMER + TCSR0, TINT);
    let periodic_ok = periodic >= 3 && top == 0xFFFF_FFFF;
    println!(
        "cascade periodic irqs={periodic} tcr1={top:#x} periodic_ok={}",
        u32::from(periodic_ok)
    );

    if capture_ok && carry_ok && periodic_ok {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    loop {
        rt::delay_us(1000);
    }
}
