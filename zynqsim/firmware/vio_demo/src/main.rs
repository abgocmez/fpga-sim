//! Virtual Input/Output demonstration (`examples/vio-demo`):
//! `vio_0` at 0x43000000 on M_AXI_GP0, `irq` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware verifies core version readout (0x03000000 for v3.0), software reset,
//! staged probe output staging and atomic commit, loopback routing, live vs latched
//! probe input sampling, activity detection with write-1-to-clear semantics,
//! autonomous stimulus generation, and interrupt delivery on input transitions.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const VIO: usize = 0x4300_0000;
const VIO_CR: usize = VIO;
const VIO_SR: usize = VIO + 0x004;
const VIO_ISR: usize = VIO + 0x008;
const VIO_IER: usize = VIO + 0x00C;

const VIO_PROBE_OUT0_VAL: usize = VIO + 0x010;
const VIO_PROBE_OUT0_STAGING: usize = VIO + 0x014;
const VIO_PROBE_OUT1_VAL: usize = VIO + 0x018;
const VIO_PROBE_OUT1_STAGING: usize = VIO + 0x01C;

const VIO_PROBE_IN0_VAL: usize = VIO + 0x020;
const VIO_PROBE_IN0_LIVE: usize = VIO + 0x024;
const VIO_PROBE_IN1_VAL: usize = VIO + 0x028;
const VIO_PROBE_IN1_LIVE: usize = VIO + 0x02C;

const VIO_PROBE_IN_ACT: usize = VIO + 0x030;
const VIO_PROBE_OUT0_INIT: usize = VIO + 0x034;
const VIO_PROBE_OUT1_INIT: usize = VIO + 0x038;
const VIO_VER: usize = VIO + 0x03C;

const CR_RESET: u32 = 1 << 0;
const CR_UPDATE_OUT: u32 = 1 << 1;
const CR_LATCH_IN: u32 = 1 << 2;
const CR_AUTO_UPDATE: u32 = 1 << 3;
const CR_LOOPBACK_EN: u32 = 1 << 4;
const CR_STIM_EN: u32 = 1 << 5;

const SR_OUT_UPDATED: u32 = 1 << 0;
const SR_IN_LATCHED: u32 = 1 << 1;

const IRQ_IN_CHANGE: u32 = 1 << 0;
const IRQ_OUT_ACK: u32 = 1 << 1;
const IRQ_ACTIVITY: u32 = 1 << 2;

const IRQ_VIO: usize = 61; // IRQ_F2P[0]

static VIO_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn vio_isr() {
    // Disable interrupts to immediately deassert level-sensitive IRQ line
    write32(VIO_IER, 0);
    let status = read32(VIO_ISR);
    write32(VIO_ISR, status); // W1C
    VIO_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vio demo: Virtual Input/Output 3.0 virtual probe monitoring and control");

    let ver = read32(VIO_VER);
    println!("VIO Version: {ver:#010x}");
    let mut pass = ver == 0x0300_0000;

    // Test software reset
    write32(VIO_PROBE_OUT0_STAGING, 0x1234_5678);
    write32(VIO_IER, 0x7);
    write32(VIO_CR, CR_RESET);
    let cr = read32(VIO_CR);
    let isr = read32(VIO_ISR);
    let ier = read32(VIO_IER);
    let sr = read32(VIO_SR);
    let num_in = (sr >> 8) & 0xFF;
    let num_out = (sr >> 16) & 0xFF;
    pass &= (cr == 0) && (isr == 0) && (ier == 0) && (num_in > 0) && (num_out > 0);
    println!("VIO software reset ok (num_in={num_in}, num_out={num_out})");

    // Verify initial reset values
    let init0 = read32(VIO_PROBE_OUT0_INIT);
    let init1 = read32(VIO_PROBE_OUT1_INIT);
    pass &= (init0 == 0) && (init1 == 0);

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_VIO, vio_isr);
    irq::enable_spi(IRQ_VIO);
    irq::enable();

    // Test staged output commit for probe 0 & 1
    write32(VIO_PROBE_OUT0_STAGING, 0xDEAD_BEEF);
    write32(VIO_PROBE_OUT1_STAGING, 0xCAFE_BABE);
    let out0_before = read32(VIO_PROBE_OUT0_VAL);
    let out1_before = read32(VIO_PROBE_OUT1_VAL);
    pass &= (out0_before == 0) && (out1_before == 0);

    // Enable output ack interrupt and commit
    write32(VIO_IER, IRQ_OUT_ACK);
    write32(VIO_CR, CR_UPDATE_OUT);
    let out0_after = read32(VIO_PROBE_OUT0_VAL);
    let out1_after = read32(VIO_PROBE_OUT1_VAL);
    let sr_comm = read32(VIO_SR);
    pass &= (out0_after == 0xDEAD_BEEF)
        && (out1_after == 0xCAFE_BABE)
        && ((sr_comm & SR_OUT_UPDATED) != 0);
    println!("VIO staged output commit ok: p0={out0_after:#010x}, p1={out1_after:#010x}");

    // Test loopback, live probe reading, and input latching
    write32(VIO_CR, CR_LOOPBACK_EN | CR_AUTO_UPDATE);
    write32(VIO_PROBE_OUT0_VAL, 0x55AA_1234);
    write32(VIO_PROBE_OUT1_VAL, 0x66BB_5678);
    let live0 = read32(VIO_PROBE_IN0_LIVE);
    let live1 = read32(VIO_PROBE_IN1_LIVE);
    let latched0_before = read32(VIO_PROBE_IN0_VAL);
    let latched1_before = read32(VIO_PROBE_IN1_VAL);
    pass &= (live0 == 0x55AA_1234)
        && (live1 == 0x66BB_5678)
        && (latched0_before == 0)
        && (latched1_before == 0);

    // Latch inputs
    write32(VIO_CR, CR_LOOPBACK_EN | CR_AUTO_UPDATE | CR_LATCH_IN);
    let latched0_after = read32(VIO_PROBE_IN0_VAL);
    let latched1_after = read32(VIO_PROBE_IN1_VAL);
    let sr_lat = read32(VIO_SR);
    pass &= (latched0_after == 0x55AA_1234)
        && (latched1_after == 0x66BB_5678)
        && ((sr_lat & SR_IN_LATCHED) != 0);
    println!("VIO loopback and input latch ok: l0={latched0_after:#010x}, l1={latched1_after:#010x}");

    // Test activity detection flag and W1C
    let act_before = read32(VIO_PROBE_IN_ACT);
    pass &= (act_before & 1) != 0;
    write32(VIO_PROBE_IN_ACT, 1); // W1C
    let act_after = read32(VIO_PROBE_IN_ACT);
    pass &= (act_after & 1) == 0;
    println!("VIO activity detection ok: act={act_before:#010x} -> {act_after:#010x}");

    // Test autonomous stimulus generator and interrupt delivery
    write32(VIO_IER, IRQ_IN_CHANGE | IRQ_ACTIVITY);
    write32(VIO_CR, CR_STIM_EN);

    let mut got_intr = false;
    for _ in 0..2000 {
        if VIO_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = VIO_INTR_COUNT.load(Ordering::SeqCst);
        let s_live = read32(VIO_PROBE_IN0_LIVE);
        println!("VIO interrupt received ({count} times), live probe: {s_live:#010x}");
        pass &= s_live != 0;

        // Stop stimulus
        write32(VIO_CR, 0);
    } else {
        println!("FAIL: VIO timed out waiting for interrupt");
        pass = false;
    }

    if pass {
        println!("VIO PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
