//! AXI4-Stream FIFO demo (`examples/axis-fifo-demo`):
//! Tests the AXI4-Stream FIFO (`xilinx.com:ip:axi_fifo_mm_s`, PG080) at 0x43C00000:
//! 1. Register reset values (ISR, IER, TDFV = TX depth, RDFO, RLR).
//! 2. Two packets written to TDFD, committed through TLR, looped back to the RX side,
//!    read back word by word through RDFD, with RLR following the packets.
//! 3. Interrupts: TC and RC on IRQ_F2P[0] (GIC 61) through IER, cleared by writing ISR.
//! 4. TDR as the packet destination (RDR), transmit underrun (TSE), TX and RX FIFO resets, SRR.
//!
//! The register facts are the model's, as remembered from PG080: see `crates/vendor-ip/src/axis_fifo.rs`
//! (its `VERIFY:` items apply to this demo too).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const FIFO: usize = 0x43C0_0000;

// Register offsets (PG080)
const ISR: usize = FIFO + 0x00;
const IER: usize = FIFO + 0x04;
const TDFR: usize = FIFO + 0x08;
const TDFV: usize = FIFO + 0x0C;
const TDFD: usize = FIFO + 0x10;
const TLR: usize = FIFO + 0x14;
const RDFR: usize = FIFO + 0x18;
const RDFO: usize = FIFO + 0x1C;
const RDFD: usize = FIFO + 0x20;
const RLR: usize = FIFO + 0x24;
const SRR: usize = FIFO + 0x28;
const TDR: usize = FIFO + 0x2C;
const RDR: usize = FIFO + 0x30;

const RESET_KEY: u32 = 0xA5;
const TX_DEPTH: u32 = 512;

const INTR_TSE: u32 = 1 << 25;
const INTR_TC: u32 = 1 << 27;
const INTR_RC: u32 = 1 << 26;

const IRQ_FIFO: usize = 61; // IRQ_F2P[0]

static FIFO_IRQS: AtomicU32 = AtomicU32::new(0);

fn fifo_isr() {
    let isr = read32(ISR);
    if isr != 0 {
        // Write 1 to clear the bits that were set.
        write32(ISR, isr);
        FIFO_IRQS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Waits (bounded) until the interrupt handler has run at least `want` times.
fn wait_for_irqs(want: u32) -> bool {
    for _ in 0..100_000u32 {
        if FIFO_IRQS.load(Ordering::SeqCst) >= want {
            return true;
        }
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("axi4-stream fifo demo");

    // 1. Reset values
    let isr0 = read32(ISR);
    let ier0 = read32(IER);
    let tdfv0 = read32(TDFV);
    let rdfo0 = read32(RDFO);
    let rlr0 = read32(RLR);
    println!("initial: isr={isr0:#x} ier={ier0:#x} tdfv={tdfv0} rdfo={rdfo0} rlr={rlr0}");
    if isr0 != 0 || ier0 != 0 || tdfv0 != TX_DEPTH || rdfo0 != 0 || rlr0 != 0 {
        println!("FAIL: bad reset values");
        rt::finish();
    }

    // 2. Interrupts: TC and RC on IRQ_F2P[0]
    irq::install();
    irq::set_handler(IRQ_FIFO, fifo_isr);
    irq::enable_spi(IRQ_FIFO);
    irq::enable();
    write32(IER, INTR_TC | INTR_RC);

    // 3. First packet: four words, 16 bytes, destination 0x3
    let first: [u32; 4] = [0x1122_3344, 0xAABB_CCDD, 0x0102_0304, 0xDEAD_BEEF];
    for &w in &first {
        write32(TDFD, w);
    }
    if read32(TDFV) != TX_DEPTH - 4 {
        println!("FAIL: TDFV after four writes is {}", read32(TDFV));
        rt::finish();
    }
    write32(TDR, 0x3);
    write32(TLR, 16);
    if read32(RDFO) != 4 || read32(RLR) != 16 {
        println!("FAIL: loopback: rdfo={} rlr={}", read32(RDFO), read32(RLR));
        rt::finish();
    }
    if !wait_for_irqs(1) {
        println!("FAIL: no AXI FIFO interrupt received");
        rt::finish();
    }
    println!("irq count after packet 1: {}", FIFO_IRQS.load(Ordering::SeqCst));
    if read32(RDR) != 0x3 {
        println!("FAIL: RDR is {:#x}, expected 0x3", read32(RDR));
        rt::finish();
    }
    for &w in &first {
        let got = read32(RDFD);
        if got != w {
            println!("FAIL: sent {w:#x}, got {got:#x}");
            rt::finish();
        }
    }
    if read32(RLR) != 0 || read32(RDFO) != 0 {
        println!("FAIL: RX FIFO not empty after the packet");
        rt::finish();
    }
    println!("packet 1: 4 words looped back");

    // 4. Second packet: two words, 6 bytes (the last word is partly used); destination 0x9
    write32(TDR, 0x9);
    write32(TDFD, 0x5555_AAAA);
    write32(TDFD, 0x0000_1234);
    write32(TLR, 6);
    if !wait_for_irqs(2) {
        println!("FAIL: no interrupt for packet 2");
        rt::finish();
    }
    if read32(RLR) != 6 || read32(RDFO) != 2 || read32(RDR) != 0x9 {
        println!(
            "FAIL: packet 2: rlr={} rdfo={} rdr={:#x}",
            read32(RLR),
            read32(RDFO),
            read32(RDR)
        );
        rt::finish();
    }
    if read32(RDFD) != 0x5555_AAAA || read32(RDFD) != 0x0000_1234 {
        println!("FAIL: packet 2 data differs");
        rt::finish();
    }
    println!("packet 2: 6 bytes looped back, dest=0x9");

    // 5. Transmit underrun: TLR asks for more words than the TX FIFO holds (interrupts off here)
    write32(IER, 0);
    write32(TDFD, 0x1);
    write32(TLR, 12);
    if read32(ISR) & INTR_TSE == 0 {
        println!("FAIL: TLR without data did not set TSE");
        rt::finish();
    }
    write32(ISR, INTR_TSE);
    write32(TDFR, RESET_KEY);
    if read32(TDFV) != TX_DEPTH {
        println!("FAIL: TDFR reset did not empty the transmit FIFO");
        rt::finish();
    }
    println!("tx reset ok");

    // 6. RX reset: one packet waiting, then RDFR
    write32(TDFD, 0x77);
    write32(TLR, 4);
    if read32(RDFO) != 1 {
        println!("FAIL: packet 3 not received");
        rt::finish();
    }
    write32(RDFR, RESET_KEY);
    if read32(RDFO) != 0 || read32(RLR) != 0 {
        println!("FAIL: RDFR reset did not empty the receive FIFO");
        rt::finish();
    }
    println!("rx reset ok");

    // 7. SRR resets the interrupt enable, the destination and the transmit FIFO
    write32(TDFD, 0x1);
    write32(SRR, RESET_KEY);
    if read32(IER) != 0 || read32(TDFV) != TX_DEPTH || read32(TDR) != 0 {
        println!("FAIL: SRR did not reset the core");
        rt::finish();
    }
    println!("srr reset ok");

    println!("RESULT: ALL PASS");
    rt::finish()
}
