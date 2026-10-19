//! Integrated Logic Analyzer demonstration (`examples/ila-demo`):
//! `ila_0` at 0x43000000 on M_AXI_GP0, `irq` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware verifies core version readout (0x06020000 for v6.2), software reset,
//! autonomous probe stimulus generation, programmable trigger matching with interrupt delivery,
//! trace buffer readout via sequential address/data ports and direct memory-mapped buffer window,
//! immediate software triggering (`CR_TRIG_NOW`), and disarm.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const ILA: usize = 0x4300_0000;
const ILA_CR: usize = ILA;
const ILA_SR: usize = ILA + 0x004;
const ILA_ISR: usize = ILA + 0x008;
const ILA_IER: usize = ILA + 0x00C;
const ILA_PROBE0_VAL: usize = ILA + 0x010;
const ILA_PROBE0_MASK: usize = ILA + 0x014;
const ILA_PROBE0_MODE: usize = ILA + 0x018;
const ILA_SAMPLE_DEPTH: usize = ILA + 0x020;
const ILA_SAMPLE_CNT: usize = ILA + 0x024;
const ILA_TRIG_POS: usize = ILA + 0x028;
const ILA_VER: usize = ILA + 0x02C;
const ILA_TRACE_RADDR: usize = ILA + 0x030;
const ILA_TRACE_RDATA: usize = ILA + 0x034;
const ILA_TRACE_BUF_BASE: usize = ILA + 0x1000;

const CR_ARM: u32 = 1 << 0;
const CR_DISARM: u32 = 1 << 1;
const CR_TRIG_NOW: u32 = 1 << 2;
const CR_RESET: u32 = 1 << 4;
const CR_STIM_EN: u32 = 1 << 5;

const SR_ARMED: u32 = 1 << 0;
const SR_TRIGGERED: u32 = 1 << 1;
const SR_IDLE: u32 = 1 << 3;

const IRQ_TRIGGERED: u32 = 1 << 0;
const IRQ_DONE: u32 = 1 << 1;

const IRQ_ILA: usize = 61; // IRQ_F2P[0]

static ILA_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn ila_isr() {
    // Disable interrupts to immediately deassert level-sensitive IRQ line
    write32(ILA_IER, 0);
    let status = read32(ILA_ISR);
    write32(ILA_ISR, status); // W1C
    ILA_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("ila demo: Integrated Logic Analyzer 6.2 trace capture and trigger verification");

    let ver = read32(ILA_VER);
    println!("ILA Version: {ver:#010x}");
    let mut pass = ver == 0x0602_0000;

    // Test software reset
    write32(ILA_PROBE0_VAL, 0x1234_5678);
    write32(ILA_IER, 0x3);
    write32(ILA_CR, CR_RESET);
    let cr = read32(ILA_CR);
    let isr = read32(ILA_ISR);
    let ier = read32(ILA_IER);
    let sr = read32(ILA_SR);
    let depth = read32(ILA_SAMPLE_DEPTH);
    pass &= (cr == 0) && (isr == 0) && (ier == 0) && ((sr & SR_IDLE) != 0) && (depth >= 16);
    println!("ILA software reset ok (depth={depth})");

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_ILA, ila_isr);
    irq::enable_spi(IRQ_ILA);
    irq::enable();

    // Configure trigger: match probe0 lower byte == 0x20
    write32(ILA_PROBE0_VAL, 0x0020);
    write32(ILA_PROBE0_MASK, 0x00FF);
    write32(ILA_PROBE0_MODE, 0); // Equal

    // Enable interrupts for trigger match and buffer done
    write32(ILA_IER, IRQ_TRIGGERED | IRQ_DONE);

    // Arm core with autonomous stimulus generation
    write32(ILA_CR, CR_ARM | CR_STIM_EN);

    // Wait for trigger match and interrupt delivery
    let mut got_intr = false;
    for _ in 0..2000 {
        if ILA_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = ILA_INTR_COUNT.load(Ordering::SeqCst);
        let sr_trig = read32(ILA_SR);
        let cnt = read32(ILA_SAMPLE_CNT);
        let pos = read32(ILA_TRIG_POS);
        println!(
            "ILA interrupt received ({count} times), status: {sr_trig:#010x}, samples: {cnt}, trig_pos: {pos}"
        );
        pass &= (sr_trig & SR_TRIGGERED) != 0 && cnt > 0;

        // Disarm to freeze acquisition and trace buffer
        write32(ILA_CR, CR_DISARM);

        // Test sequential trace readout via address/data port
        write32(ILA_TRACE_RADDR, 0);
        let sample0 = read32(ILA_TRACE_RDATA);
        let sample1 = read32(ILA_TRACE_RDATA);
        let raddr = read32(ILA_TRACE_RADDR);
        pass &= raddr == 2;
        println!(
            "ILA sequential readout ok: s0={sample0:#010x}, s1={sample1:#010x}, raddr={raddr}"
        );

        // Test direct memory-mapped buffer window access
        let direct0 = read32(ILA_TRACE_BUF_BASE);
        let direct1 = read32(ILA_TRACE_BUF_BASE + 4);
        pass &= (direct0 == sample0) && (direct1 == sample1);
        println!("ILA direct buffer window ok: d0={direct0:#010x}, d1={direct1:#010x}");
    } else {
        println!("FAIL: ILA timed out waiting for interrupt");
        pass = false;
    }

    // Test immediate trigger
    write32(ILA_CR, CR_ARM);
    let sr_armed = read32(ILA_SR);
    write32(ILA_CR, CR_TRIG_NOW);
    let sr_trig = read32(ILA_SR);
    write32(ILA_CR, CR_DISARM);
    let sr_idle = read32(ILA_SR);
    pass &= ((sr_armed & SR_ARMED) != 0)
        && ((sr_trig & SR_TRIGGERED) != 0)
        && ((sr_idle & SR_IDLE) != 0);
    println!("ILA immediate trigger test ok");

    if pass {
        println!("ILA PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
