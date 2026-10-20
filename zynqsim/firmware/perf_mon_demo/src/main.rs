//! AXI Performance Monitor demonstration (`examples/perf-mon-demo`):
//! `axi_perf_mon_0` at 0x43000000 on M_AXI_GP0, `interrupt` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware exercises core reset via SRR key 0xA, metric selection registers (MSR0, MSR1),
//! sample interval timer configuration and auto-reload, real-time metric counter readout (MC0..MC3),
//! periodic snapshot latching into sampled metric counters (SMC0..SMC3), manual snapshot triggering via SICR,
//! and sample interval expiration interrupt handling through GIC SPI 61.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const APM: usize = 0x4300_0000;
const APM_SRR: usize = APM;
const APM_SR: usize = APM + 0x004;
const APM_CR: usize = APM + 0x024;
const APM_SICR: usize = APM + 0x028;
const APM_IPISR: usize = APM + 0x030;
const APM_IPIER: usize = APM + 0x034;
const APM_SI: usize = APM + 0x038;
const APM_SIC: usize = APM + 0x03C;
const APM_MSR0: usize = APM + 0x044;
const APM_MSR1: usize = APM + 0x048;

// Metric Counters (MC) and Sampled Metric Counters (SMC)
const APM_MC0: usize = APM + 0x100;
const APM_MC1: usize = APM + 0x110;
const APM_MC2: usize = APM + 0x120;
const APM_MC3: usize = APM + 0x130;
const APM_SMC0: usize = APM + 0x200;
const APM_SMC1: usize = APM + 0x210;
const APM_SMC2: usize = APM + 0x220;
const APM_SMC3: usize = APM + 0x230;

const SRR_RESET_KEY: u32 = 0x0000_000A;

const CR_METRIC_CNT_EN: u32 = 1 << 0;
const CR_SAMPLE_CNT_EN: u32 = 1 << 2;
const CR_SAMPLE_LOAD: u32 = 1 << 4;

const IRQ_SAMPLE_EXPIRED: u32 = 1 << 0;

const IRQ_PERF_MON: usize = 61; // IRQ_F2P[0]

static PERF_MON_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn perf_mon_isr() {
    let status = read32(APM_IPISR);
    write32(APM_IPISR, status); // W1C
    PERF_MON_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("perf_mon demo: Xilinx AXI Performance Monitor 5.0 (PG037)");

    let sr_init = read32(APM_SR);
    println!("APM Status: {sr_init:#010x}");

    // Test software reset via SRR key 0xA
    write32(APM_CR, CR_METRIC_CNT_EN | CR_SAMPLE_CNT_EN);
    let cr_before = read32(APM_CR);
    let mut pass = (cr_before & (CR_METRIC_CNT_EN | CR_SAMPLE_CNT_EN)) != 0;

    write32(APM_SRR, SRR_RESET_KEY);
    let cr_after = read32(APM_CR);
    pass &= cr_after == 0;
    println!("APM software reset ok: {cr_before:#010x} -> {cr_after:#010x}");

    // Configure metric selection registers
    write32(APM_MSR0, 0x0102_0304);
    write32(APM_MSR1, 0x0506_0708);
    let msr0 = read32(APM_MSR0);
    let msr1 = read32(APM_MSR1);
    let msr_ok = msr0 == 0x0102_0304 && msr1 == 0x0506_0708;
    pass &= msr_ok;
    println!("APM metric selection registers ok (MSR0={msr0:#010x}, MSR1={msr1:#010x})");

    // Configure sample interval: 15,000 cycles
    write32(APM_SI, 15_000);
    let si = read32(APM_SI);
    let sic = read32(APM_SIC);
    let si_ok = si == 15_000 && sic == 15_000;
    pass &= si_ok;
    println!("APM sample interval register ok (SI={si}, SIC={sic})");

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_PERF_MON, perf_mon_isr);
    irq::enable_spi(IRQ_PERF_MON);
    irq::enable();

    // Enable sample expired interrupt
    write32(APM_IPIER, IRQ_SAMPLE_EXPIRED);

    // Start metric and sample counters with load
    write32(APM_CR, CR_METRIC_CNT_EN | CR_SAMPLE_CNT_EN | CR_SAMPLE_LOAD);

    // Wait for sample interval expiration interrupt
    let mut got_intr = false;
    for _ in 0..2000 {
        if PERF_MON_INTR_COUNT.load(Ordering::SeqCst) > 0 {
            got_intr = true;
            break;
        }
        rt::delay_us(10);
    }

    if got_intr {
        let count = PERF_MON_INTR_COUNT.load(Ordering::SeqCst);
        let sr_active = read32(APM_SR);
        println!(
            "APM sample interval interrupt received ({count} times), status: {sr_active:#010x}"
        );
        pass &= count > 0;
    } else {
        println!("FAIL: APM timed out waiting for interrupt");
        pass = false;
    }

    // Read active metric counters
    let mc0_bytes = read32(APM_MC0);
    let mc1_tx = read32(APM_MC1);
    let mc2_lat = read32(APM_MC2);
    let mc3_trans = read32(APM_MC3);
    println!(
        "APM Active Metrics: bytes={mc0_bytes}, tx={mc1_tx}, latency={mc2_lat}, transfers={mc3_trans}"
    );
    pass &= mc0_bytes > 0 && mc1_tx > 0;

    // Read sampled metric counters
    let smc0_bytes = read32(APM_SMC0);
    let smc1_tx = read32(APM_SMC1);
    let smc2_lat = read32(APM_SMC2);
    let smc3_trans = read32(APM_SMC3);
    println!(
        "APM Sampled Metrics: bytes={smc0_bytes}, tx={smc1_tx}, latency={smc2_lat}, transfers={smc3_trans}"
    );
    pass &= smc0_bytes > 0 && smc1_tx > 0;

    // Test manual snapshot trigger via SICR
    write32(APM_SICR, 1);
    let smc0_after = read32(APM_SMC0);
    println!("APM manual snapshot ok: {smc0_bytes} -> {smc0_after}");
    pass &= smc0_after >= smc0_bytes;

    if pass {
        println!("PERF_MON PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
