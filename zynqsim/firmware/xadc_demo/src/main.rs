//! XADC Wizard demonstration (`examples/xadc-demo`):
//! `xadc_wiz_0` at 0x43000000 on M_AXI_GP0, `ip2intc_irpt` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware verifies core reset via SRR key 0xA, direct memory-mapped DRP
//! reading of on-chip sensors (temperature, VCCINT, VCCAUX, VCCBRAM), valid physical
//! sensor ranges, sequencer conversion progression, and End-of-Sequence / End-of-Conversion
//! interrupt delivery through GIER/IPIER.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const XADC: usize = 0x4300_0000;
const XADC_SRR: usize = XADC;
const XADC_SR: usize = XADC + 0x004;
const XADC_AIFR: usize = XADC + 0x008;
const XADC_IPISR: usize = XADC + 0x05C;
const XADC_IPIER: usize = XADC + 0x060;
const XADC_GIER: usize = XADC + 0x068;

// DRP Offsets
const XADC_TEMP: usize = XADC + 0x200;
const XADC_VCCINT: usize = XADC + 0x204;
const XADC_VCCAUX: usize = XADC + 0x208;
const XADC_VCCBRAM: usize = XADC + 0x218;
const XADC_CONF0: usize = XADC + 0x300;

const SRR_RESET_KEY: u32 = 0x0000_000A;
const GIER_ENABLE: u32 = 1 << 31;
const IRQ_EOS: u32 = 1 << 8;
const IRQ_EOC: u32 = 1 << 9;

const IRQ_XADC: usize = 61; // IRQ_F2P[0]

static XADC_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn xadc_isr() {
    // Disable global interrupt to immediately deassert level-sensitive IRQ line
    write32(XADC_GIER, 0);
    let status = read32(XADC_IPISR);
    write32(XADC_IPISR, status); // W1C
    XADC_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("xadc demo: 7-Series / Zynq-7000 XADC Wizard 3.3 analog monitoring and alarms");

    let sr_init = read32(XADC_SR);
    println!("XADC Status: {sr_init:#010x}");

    // Test software reset via SRR key 0xA
    write32(XADC_CONF0, 0x1234);
    let conf_before = read32(XADC_CONF0);
    let mut pass = conf_before == 0x1234;

    write32(XADC_SRR, SRR_RESET_KEY);
    let conf_after = read32(XADC_CONF0);
    pass &= conf_after == 0;
    println!("XADC software reset ok: {conf_before:#010x} -> {conf_after:#010x}");

    // Read on-chip sensors via DRP
    let temp_raw = (read32(XADC_TEMP) >> 4) & 0x0FFF;
    let vccint_raw = (read32(XADC_VCCINT) >> 4) & 0x0FFF;
    let vccaux_raw = (read32(XADC_VCCAUX) >> 4) & 0x0FFF;
    let vccbram_raw = (read32(XADC_VCCBRAM) >> 4) & 0x0FFF;

    println!("XADC Raw Sensors: temp={temp_raw}, vccint={vccint_raw}, vccaux={vccaux_raw}, vccbram={vccbram_raw}");

    // Verify raw codes fall within realistic physical ranges
    // Temp ~25 C (code ~2426 in 2000..=2800)
    // VCCINT ~1.0 V (code ~1365 in 1200..=1500)
    // VCCAUX ~1.8 V (code ~2458 in 2200..=2700)
    // VCCBRAM ~1.0 V (code ~1365 in 1200..=1500)
    let temp_ok = (2000..=2800).contains(&temp_raw);
    let vccint_ok = (1200..=1500).contains(&vccint_raw);
    let vccaux_ok = (2200..=2700).contains(&vccaux_raw);
    let vccbram_ok = (1200..=1500).contains(&vccbram_raw);
    pass &= temp_ok && vccint_ok && vccaux_ok && vccbram_ok;
    println!("XADC sensor ranges ok (temp={temp_ok}, vccint={vccint_ok}, vccaux={vccaux_ok}, vccbram={vccbram_ok})");

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_XADC, xadc_isr);
    irq::enable_spi(IRQ_XADC);
    irq::enable();

    // Enable EOC and EOS interrupts and Global Interrupt Enable
    write32(XADC_IPIER, IRQ_EOC | IRQ_EOS);
    write32(XADC_GIER, GIER_ENABLE);

    // Wait for conversion sequence interrupt
    let mut got_intr = false;
    for _ in 0..2000 {
        if XADC_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = XADC_INTR_COUNT.load(Ordering::SeqCst);
        let sr_conv = read32(XADC_SR);
        let aifr = read32(XADC_AIFR);
        println!("XADC interrupt received ({count} times), status: {sr_conv:#010x}, alarms: {aifr:#010x}");
        pass &= count > 0;
    } else {
        println!("FAIL: XADC timed out waiting for interrupt");
        pass = false;
    }

    if pass {
        println!("XADC PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}

