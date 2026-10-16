//! AXI Interrupt Controller demo (`examples/intc-demo`, `fixtures/xsa/zs_intc.xsa`):
//! `axi_intc_0` at 0x41800000 collects the AXI GPIO's interrupt (input 0, `axi_gpio_0` at
//! 0x41200000, four switches) and the AXI Timer's (input 1, `axi_timer_0` at 0x42800000) and
//! raises IRQ_F2P[0] (GIC 61).
//!
//! 1. software interrupt: `MER = ME` only, write `ISR`, take the interrupt, acknowledge;
//! 2. `MER = ME | HIE`: three timer interrupts (periodic timer, 50 us), served through `IVR`
//!    and acknowledged with `IAR`;
//! 3. forever: every switch change is a GPIO interrupt reported on UART1.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const INTC: usize = 0x4180_0000;
const ISR: usize = INTC;
const IPR: usize = INTC + 0x04;
const IER: usize = INTC + 0x08;
const IAR: usize = INTC + 0x0C;
const SIE: usize = INTC + 0x10;
const CIE: usize = INTC + 0x14;
const IVR: usize = INTC + 0x18;
const MER: usize = INTC + 0x1C;
const GPIO: usize = 0x4120_0000;
const GPIO_DATA: usize = GPIO;
const GPIO_GIER: usize = GPIO + 0x11C;
const GPIO_ISR: usize = GPIO + 0x120;
const GPIO_IER: usize = GPIO + 0x128;
const TIMER: usize = 0x4280_0000;
const TCSR0: usize = TIMER;
const TLR0: usize = TIMER + 0x04;

const UDT: u32 = 1 << 1;
const ARHT: u32 = 1 << 4;
const LOAD: u32 = 1 << 5;
const ENIT: u32 = 1 << 6;
const ENT: u32 = 1 << 7;
const TINT: u32 = 1 << 8;

const IRQ_INTC: usize = 61; // IRQ_F2P[0]

static TICKS: AtomicU32 = AtomicU32::new(0);
static GPIO_EVENTS: AtomicU32 = AtomicU32::new(0);
static LAST_SW: AtomicU32 = AtomicU32::new(0);
static ORDER_ERRORS: AtomicU32 = AtomicU32::new(0);
static MAX_ISR_PASSES: AtomicU32 = AtomicU32::new(0);

/// Serves every pending input, lowest number first (IVR), like the Xilinx driver's handler.
fn intc_isr() {
    let mut passes = 0;
    loop {
        let v = read32(IVR);
        if v == 0xFFFF_FFFF {
            break;
        }
        passes += 1;
        match v {
            0 => {
                // The GPIO: acknowledge at the source (toggle on write), then at the controller.
                let isr = read32(GPIO_ISR);
                write32(GPIO_ISR, isr);
                LAST_SW.store(read32(GPIO_DATA) & 0xF, Ordering::SeqCst);
                GPIO_EVENTS.fetch_add(1, Ordering::SeqCst);
            }
            1 => {
                // The timer: clear TINT by writing it back.
                write32(TCSR0, read32(TCSR0));
                TICKS.fetch_add(1, Ordering::SeqCst);
            }
            _ => {
                ORDER_ERRORS.fetch_add(1, Ordering::SeqCst);
            }
        }
        write32(IAR, 1 << v);
        if passes > 8 {
            ORDER_ERRORS.fetch_add(1, Ordering::SeqCst);
            break;
        }
    }
    if passes > MAX_ISR_PASSES.load(Ordering::SeqCst) {
        MAX_ISR_PASSES.store(passes, Ordering::SeqCst);
    }
}

fn wait_for(what: &AtomicU32, n: u32) -> bool {
    for _ in 0..3000 {
        if what.load(Ordering::SeqCst) >= n {
            return true;
        }
        rt::delay_us(10);
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("intc demo: AXI Interrupt Controller, GPIO on input 0, timer on input 1");

    irq::install();
    irq::set_handler(IRQ_INTC, intc_isr);
    irq::enable_spi(IRQ_INTC);
    irq::enable();

    // 1. Software interrupt: with only ME set the controller ignores its inputs, and a write to
    //    ISR makes an input pending. Input 1 (the timer, which is not running) is used; the
    //    handler clears the timer's flag harmlessly and acknowledges it in IAR.
    write32(MER, 1); // ME only
    write32(SIE, 1 << 1);
    write32(ISR, 1 << 1);
    let soft_ok = wait_for(&TICKS, 1);
    TICKS.store(0, Ordering::SeqCst);
    println!(
        "software interrupt: taken={soft_ok} isr after ack={:#x} mer={:#x}",
        read32(ISR),
        read32(MER)
    );

    // 2. Hardware interrupts.
    write32(MER, 3);
    write32(GPIO_IER, 1);
    write32(GPIO_GIER, 0x8000_0000);
    write32(SIE, 1 << 1);
    println!("ier={:#x} mer={:#x}", read32(IER), read32(MER));

    // Timer: 5000 clocks (50 us at 100 MHz), periodic, down counter.
    write32(TLR0, 4999);
    write32(TCSR0, LOAD);
    write32(TCSR0, ENT | ENIT | ARHT | UDT);
    let ok = wait_for(&TICKS, 3);
    write32(TCSR0, TINT); // stop and clear
    // Let a pending tick be served before reading the count.
    rt::delay_us(100);
    let ticks = TICKS.load(Ordering::SeqCst);
    println!(
        "timer ticks={ticks} (via input 1) ipr={:#x} errors={}",
        read32(IPR),
        ORDER_ERRORS.load(Ordering::SeqCst)
    );
    // CIE masks an input again.
    write32(CIE, 1 << 1);
    println!("ier after CIE={:#x}", read32(IER));

    if ok && soft_ok && ORDER_ERRORS.load(Ordering::SeqCst) == 0 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    // 3. Report switch changes.
    let mut seen = 0;
    loop {
        let n = GPIO_EVENTS.load(Ordering::SeqCst);
        if n != seen {
            seen = n;
            println!(
                "gpio interrupt #{n}: switches={:#x} (max isr passes {})",
                LAST_SW.load(Ordering::SeqCst),
                MAX_ISR_PASSES.load(Ordering::SeqCst)
            );
        }
        rt::delay_us(50);
    }
}
