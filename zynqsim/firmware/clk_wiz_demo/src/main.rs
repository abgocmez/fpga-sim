//! Clocking Wizard demo (`examples/clk-wiz-demo`, ADR-042):
//! Drives the Clocking Wizard (`xilinx.com:ip:clk_wiz:6.0`, PG065) at 0x43C00000 and checks:
//! 1. LOCKED rises after power-on, and the clocking registers keep what is written.
//! 2. A dynamic reconfiguration (SADDR = 1) drops LOCKED and LOCKED rises again.
//! 3. The lock-change interrupt (IER/ISR) reaches the PS on IRQ_F2P[1] (GIC 62).
//! 4. A software reset (SRR key 0xA) clears the configuration and relocks.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const CLK: usize = 0x43C0_0000;

// Register offsets (PG065; VERIFY against the product guide, see crates/vendor-ip/src/clk_wiz.rs)
const SRR: usize = CLK + 0x000;
const SR: usize = CLK + 0x004;
const ISR: usize = CLK + 0x00C;
const IER: usize = CLK + 0x010;
const CCR0: usize = CLK + 0x200;
const CCR2: usize = CLK + 0x208;
const SADDR: usize = CLK + 0x25C;

const SRR_RESET_KEY: u32 = 0x0000_000A;
const SR_LOCKED: u32 = 1 << 0;
const INTR_LOCK_CHANGED: u32 = 1 << 0;
const SADDR_LOAD: u32 = 1 << 0;

const IRQ_CLK: usize = 62; // IRQ_F2P[1]

/// Bounded wait for a SR state; each poll is a bus access, which advances simulated time.
const POLLS: u32 = 200_000;

static LOCK_IRQS: AtomicU32 = AtomicU32::new(0);

fn clk_isr() {
    let isr = read32(ISR);
    if isr != 0 {
        // Write 1 to clear.
        write32(ISR, isr);
        LOCK_IRQS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Polls until `LOCKED` equals `want`; false when it does not happen in time.
fn wait_locked(want: bool) -> bool {
    for _ in 0..POLLS {
        if ((read32(SR) & SR_LOCKED) != 0) == want {
            return true;
        }
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("clk-wiz demo");

    // 1. Power-on lock and register retention
    if !wait_locked(true) {
        println!("FAIL: not locked after power-on");
        rt::finish();
    }
    println!("power-on: locked sr={:#x}", read32(SR));
    write32(CCR0, 0x0A01); // DIVCLK 1, CLKFBOUT mult 10
    write32(CCR2, 0x0A); // CLKOUT0 divide 10
    if read32(CCR0) != 0x0A01 || read32(CCR2) != 0x0A {
        println!("FAIL: clocking registers do not keep their values");
        rt::finish();
    }
    println!("config ok");

    // 2. Interrupt on lock changes
    irq::install();
    irq::set_handler(IRQ_CLK, clk_isr);
    irq::enable_spi(IRQ_CLK);
    irq::enable();
    write32(IER, INTR_LOCK_CHANGED);

    // 3. Dynamic reconfiguration: LOCKED falls at once and rises again
    write32(SADDR, SADDR_LOAD);
    if (read32(SR) & SR_LOCKED) != 0 {
        println!("FAIL: LOCKED still set right after reconfiguration");
        rt::finish();
    }
    println!("reconfig: unlocked");
    if !wait_locked(true) {
        println!("FAIL: no relock after reconfiguration");
        rt::finish();
    }
    println!("reconfig: relocked");

    // The interrupt is delivered through the GIC; wait for its handler.
    let mut polls = 0;
    while LOCK_IRQS.load(Ordering::SeqCst) == 0 && polls < POLLS {
        polls += 1;
    }
    let irq_count = LOCK_IRQS.load(Ordering::SeqCst);
    println!("clk irq count: {irq_count}");
    if irq_count == 0 {
        println!("FAIL: no lock-change interrupt");
        rt::finish();
    }

    // 4. Software reset clears the configuration and relocks
    write32(IER, 0);
    write32(SRR, SRR_RESET_KEY);
    if read32(CCR0) != 0 || read32(IER) != 0 {
        println!("FAIL: SRR did not clear the configuration");
        rt::finish();
    }
    if !wait_locked(true) {
        println!("FAIL: no relock after SRR");
        rt::finish();
    }
    println!("srr reset ok");

    println!("RESULT: ALL PASS");
    rt::finish()
}
