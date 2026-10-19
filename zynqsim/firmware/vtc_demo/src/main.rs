//! Video Timing Controller demonstration (`examples/vtc-demo`):
//! `v_tc_0` at 0x43000000 on M_AXI_GP0, `irq` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests version readout, software reset, video timing generator
//! configuration (640x480 active resolution), frame sync and VBLANK interrupts,
//! and verifies completion.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const VTC: usize = 0x4300_0000;
const VTC_CTL: usize = VTC;
const VTC_ISR: usize = VTC + 0x004;
const VTC_IER: usize = VTC + 0x00C;
const VTC_VER: usize = VTC + 0x010;

const VTC_GASIZE: usize = VTC + 0x060;
const VTC_GTSTAT: usize = VTC + 0x064;
const VTC_GHSIZE: usize = VTC + 0x070;
const VTC_GVSIZE: usize = VTC + 0x074;
const VTC_GHSYNC: usize = VTC + 0x078;
const VTC_GVSYNC: usize = VTC + 0x080;

const CTL_RESET: u32 = 1 << 31;
const CTL_GEN_EN: u32 = 1 << 2;
const CTL_CORE_EN: u32 = 1 << 0;

const IXR_FSYNC0: u32 = 1 << 16;
const IXR_G_VBLANK: u32 = 1 << 12;

const IRQ_VTC: usize = 61; // IRQ_F2P[0]

static VTC_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn vtc_isr() {
    // Disable interrupts to deassert level-sensitive IRQ line immediately
    write32(VTC_IER, 0);
    let status = read32(VTC_ISR);
    write32(VTC_ISR, status); // W1C
    VTC_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vtc demo: Video Timing Controller generator and interrupt demo");

    let ver = read32(VTC_VER);
    println!("VTC Version: {ver:#010x}");
    let mut pass = ver == 0x0602_0000;

    // Test software reset
    write32(VTC_GASIZE, 0x01E0_0280);
    write32(VTC_CTL, CTL_RESET);
    let ctl = read32(VTC_CTL);
    let gasize = read32(VTC_GASIZE);
    pass &= (ctl == 0) && (gasize == 0);

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_VTC, vtc_isr);
    irq::enable_spi(IRQ_VTC);
    irq::enable();

    // Configure 640x480 timing: HTotal=800, VTotal=525
    write32(VTC_GASIZE, (480 << 16) | 640);
    write32(VTC_GHSIZE, 800);
    write32(VTC_GVSIZE, 525);
    write32(VTC_GHSYNC, (752 << 16) | 656);
    write32(VTC_GVSYNC, (492 << 16) | 490);

    // Enable interrupts for FSYNC0 and G_VBLANK
    write32(VTC_IER, IXR_FSYNC0 | IXR_G_VBLANK);

    // Enable Generator and Core
    write32(VTC_CTL, CTL_CORE_EN | CTL_GEN_EN);

    // Wait for frame generation and interrupt delivery
    let mut got_intr = false;
    for _ in 0..2000 {
        if VTC_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = VTC_INTR_COUNT.load(Ordering::SeqCst);
        let tstat = read32(VTC_GTSTAT);
        println!("VTC frame interrupt received ({count} times), timing status: {tstat:#x}");
    } else {
        println!("FAIL: VTC generator timed out waiting for interrupt");
        pass = false;
    }

    if pass {
        println!("VTC PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
