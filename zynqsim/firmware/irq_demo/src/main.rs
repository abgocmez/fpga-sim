//! Interrupt demo for the `irq_ip` PL block: the IP raises a level-high
//! interrupt on IRQ_F2P[0] (GIC ID 61) when triggered; the handler reads the
//! status, acknowledges it and counts. Three triggers must give three events.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const IP: usize = 0x43C0_0000;
const CTRL: usize = IP;
const TRIGGER: usize = IP + 0x04;
const STATUS: usize = IP + 0x08;
const ACK: usize = IP + 0x0C;

const IRQ_F2P0: usize = 61;

static HANDLED: AtomicU32 = AtomicU32::new(0);
static SPURIOUS: AtomicU32 = AtomicU32::new(0);

fn isr() {
    // A level interrupt can be taken once more if the line has not fallen by
    // the time it is ended: count only when the IP really had one pending.
    if read32(STATUS) & 1 != 0 {
        write32(ACK, 1);
        HANDLED.fetch_add(1, Ordering::SeqCst);
    } else {
        SPURIOUS.fetch_add(1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("irq demo: IRQ_F2P[0] = GIC {IRQ_F2P0}, level high");

    irq::install();
    irq::set_handler(IRQ_F2P0, isr);
    irq::enable_spi(IRQ_F2P0);
    write32(CTRL, 1); // interrupt enable in the IP
    irq::enable();

    let mut ok = true;
    for n in 1..=3u32 {
        write32(TRIGGER, 1);
        let mut waited = 0;
        while HANDLED.load(Ordering::SeqCst) < n {
            rt::delay_us(10);
            waited += 1;
            if waited > 2000 {
                println!("trigger {n}: no interrupt after 20 ms");
                ok = false;
                break;
            }
        }
        println!("trigger {n}: handled={}", HANDLED.load(Ordering::SeqCst));
        if !ok {
            break;
        }
    }
    let events = read32(STATUS) >> 8;
    println!(
        "events={events} handled={} spurious={}",
        HANDLED.load(Ordering::SeqCst),
        SPURIOUS.load(Ordering::SeqCst)
    );
    if ok && events == 3 && HANDLED.load(Ordering::SeqCst) == 3 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    rt::finish()
}
