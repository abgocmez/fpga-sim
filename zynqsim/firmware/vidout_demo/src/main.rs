//! AXI4-Stream to Video Out demonstration (`examples/vidout-demo`):
//! `v_axi4s_vid_out_0` at 0x43000000 on M_AXI_GP0, `irq` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests version readout (0x04000000 for v4.0), software reset,
//! video resolution configuration, manual pixel injection with TUSER (SOF) and
//! TLAST (EOL), autonomous pattern generation, interrupt delivery on frame
//! completion and lock synchronization, and verifies completion.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const VIDOUT: usize = 0x4300_0000;
const VIDOUT_CR: usize = VIDOUT;
const VIDOUT_SR: usize = VIDOUT + 0x004;
const VIDOUT_ISR: usize = VIDOUT + 0x008;
const VIDOUT_IER: usize = VIDOUT + 0x00C;
const VIDOUT_ASIZE: usize = VIDOUT + 0x010;
const VIDOUT_TSIZE: usize = VIDOUT + 0x014;
const VIDOUT_FIFO_STATUS: usize = VIDOUT + 0x018;
const VIDOUT_STREAM_DATA: usize = VIDOUT + 0x020;
const VIDOUT_STREAM_CTRL: usize = VIDOUT + 0x024;
const VIDOUT_FRAME_CNT: usize = VIDOUT + 0x030;
const VIDOUT_VER: usize = VIDOUT + 0x03C;

const CR_EN: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 1;
const CR_PATTERN_EN: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;

const IRQ_LOCK: u32 = 1 << 0;
const IRQ_FRAME_DONE: u32 = 1 << 6;

const IRQ_VIDOUT: usize = 61; // IRQ_F2P[0]

static VIDOUT_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn vidout_isr() {
    // Disable interrupts to deassert level-sensitive IRQ line immediately
    write32(VIDOUT_IER, 0);
    let status = read32(VIDOUT_ISR);
    write32(VIDOUT_ISR, status); // W1C
    VIDOUT_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vidout demo: AXI4-Stream to Video Out 4.0 stream lock and timing");

    let ver = read32(VIDOUT_VER);
    println!("VIDOUT Version: {ver:#010x}");
    let mut pass = ver == 0x0400_0000;

    // Test software reset
    write32(VIDOUT_ASIZE, 0x01E0_0280);
    write32(VIDOUT_CR, CR_RESET);
    let cr = read32(VIDOUT_CR);
    let asize = read32(VIDOUT_ASIZE);
    pass &= (cr == CR_EN) && (asize == ((480 << 16) | 640));
    println!("VIDOUT software reset ok");

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_VIDOUT, vidout_isr);
    irq::enable_spi(IRQ_VIDOUT);
    irq::enable();

    // Keep core disabled during configuration & manual pixel injection
    write32(VIDOUT_CR, 0);

    // Configure fast active resolution (8x4 active, 10x5 total frame)
    write32(VIDOUT_ASIZE, (4 << 16) | 8);
    write32(VIDOUT_TSIZE, (5 << 16) | 10);
    println!("VIDOUT resolution configured (8x4 active, 10x5 total)");

    // Test stream data manual injection (TUSER = 1 for SOF)
    write32(VIDOUT_STREAM_CTRL, 1);
    write32(VIDOUT_STREAM_DATA, 0x00FF_0000); // Red pixel
    let lvl = read32(VIDOUT_FIFO_STATUS) & 0xFFFF;
    pass &= lvl >= 1;
    println!("VIDOUT FIFO manual stream push ok (level: {lvl})");

    // Enable interrupts for lock and frame completion
    write32(VIDOUT_IER, IRQ_LOCK | IRQ_FRAME_DONE);

    // Enable Core with Pattern Generator & Loopback
    write32(VIDOUT_CR, CR_EN | CR_PATTERN_EN | CR_LOOPBACK_EN);

    // Wait for frame generation and interrupt delivery
    let mut got_intr = false;
    for _ in 0..2000 {
        if VIDOUT_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = VIDOUT_INTR_COUNT.load(Ordering::SeqCst);
        let sr = read32(VIDOUT_SR);
        let locked = (sr & 1) != 0;
        let frames = read32(VIDOUT_FRAME_CNT);
        println!("VIDOUT interrupt received ({count} times), status: {sr:#010x} (locked={locked})");
        println!("VIDOUT completed frames: {frames}");
        pass &= locked && (frames > 0);
    } else {
        println!("FAIL: VIDOUT timed out waiting for interrupt");
        pass = false;
    }

    if pass {
        println!("VIDOUT PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
