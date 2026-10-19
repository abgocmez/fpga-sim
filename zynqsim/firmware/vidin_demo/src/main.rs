//! Video In to AXI4-Stream demonstration (`examples/vidin-demo`):
//! `v_vid_in_axi4s_0` at 0x43000000 on M_AXI_GP0, `irq` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests version readout (0x04000000 for v4.0), software reset,
//! video resolution configuration, autonomous pattern generation, interrupt delivery
//! on frame completion and lock synchronization, stream FIFO readout and pop,
//! and verifies completion.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const VIDIN: usize = 0x4300_0000;
const VIDIN_CR: usize = VIDIN;
const VIDIN_SR: usize = VIDIN + 0x004;
const VIDIN_ISR: usize = VIDIN + 0x008;
const VIDIN_IER: usize = VIDIN + 0x00C;
const VIDIN_ASIZE: usize = VIDIN + 0x010;
const VIDIN_TSIZE: usize = VIDIN + 0x014;
const VIDIN_FIFO_STATUS: usize = VIDIN + 0x018;
const VIDIN_STREAM_DATA: usize = VIDIN + 0x020;
const VIDIN_STREAM_CTRL: usize = VIDIN + 0x024;
const VIDIN_FRAME_CNT: usize = VIDIN + 0x030;
const VIDIN_VER: usize = VIDIN + 0x03C;

const CR_EN: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 1;
const CR_PATTERN_EN: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;

const IRQ_LOCK: u32 = 1 << 0;
const IRQ_FRAME_DONE: u32 = 1 << 6;

const IRQ_VIDIN: usize = 61; // IRQ_F2P[0]

static VIDIN_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn vidin_isr() {
    // Disable interrupts to deassert level-sensitive IRQ line immediately
    write32(VIDIN_IER, 0);
    let status = read32(VIDIN_ISR);
    write32(VIDIN_ISR, status); // W1C
    VIDIN_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vidin demo: Video In to AXI4-Stream 4.0 frame capture and timing");

    let ver = read32(VIDIN_VER);
    println!("VIDIN Version: {ver:#010x}");
    let mut pass = ver == 0x0400_0000;

    // Test software reset
    write32(VIDIN_ASIZE, 0x01E0_0280);
    write32(VIDIN_IER, 0x1F);
    write32(VIDIN_CR, CR_RESET);
    let cr = read32(VIDIN_CR);
    let asize = read32(VIDIN_ASIZE);
    let ier = read32(VIDIN_IER);
    pass &= (cr == CR_EN) && (asize == ((480 << 16) | 640)) && (ier == 0);
    println!("VIDIN software reset ok");

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_VIDIN, vidin_isr);
    irq::enable_spi(IRQ_VIDIN);
    irq::enable();

    // Keep core disabled during configuration
    write32(VIDIN_CR, 0);

    // Configure fast active resolution (8x4 active, 10x5 total frame)
    write32(VIDIN_ASIZE, (4 << 16) | 8);
    write32(VIDIN_TSIZE, (5 << 16) | 10);
    println!("VIDIN resolution configured (8x4 active, 10x5 total)");

    // Enable interrupts for lock and frame completion
    write32(VIDIN_IER, IRQ_LOCK | IRQ_FRAME_DONE);

    // Enable Core with Pattern Generator & Loopback
    write32(VIDIN_CR, CR_EN | CR_PATTERN_EN | CR_LOOPBACK_EN);

    // Wait for frame capture and interrupt delivery
    let mut got_intr = false;
    for _ in 0..2000 {
        if VIDIN_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = VIDIN_INTR_COUNT.load(Ordering::SeqCst);
        let sr = read32(VIDIN_SR);
        let locked = (sr & 1) != 0;
        let frames = read32(VIDIN_FRAME_CNT);
        println!("VIDIN interrupt received ({count} times), status: {sr:#010x} (locked={locked})");
        println!("VIDIN completed frames: {frames}");
        pass &= locked && (frames > 0);

        // Pause incoming video generation so FIFO level remains stable during verification
        write32(VIDIN_CR, 0);

        // Test stream FIFO inspection and pop
        let lvl = read32(VIDIN_FIFO_STATUS) & 0xFFFF;
        let ctrl = read32(VIDIN_STREAM_CTRL);
        let tvalid = (ctrl & 1) != 0;
        let px = read32(VIDIN_STREAM_DATA);
        write32(VIDIN_STREAM_CTRL, 8); // Pop
        let new_lvl = read32(VIDIN_FIFO_STATUS) & 0xFFFF;
        pass &= tvalid && (lvl > 0) && (new_lvl == lvl - 1);
        println!("VIDIN stream pop ok (level: {lvl} -> {new_lvl}, pixel: {px:#010x})");
    } else {
        println!("FAIL: VIDIN timed out waiting for interrupt");
        pass = false;
    }

    if pass {
        println!("VIDIN PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
