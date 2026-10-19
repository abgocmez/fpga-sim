//! AXI Quad SPI demo (`examples/spi-demo`):
//! Tests the AXI Quad SPI core (`xilinx.com:ip:axi_quad_spi:3.2`, PG153) at 0x41E00000:
//! 1. Soft reset via SRR and register reset verification (SPICR=0x0180, SPISR=0x05).
//! 2. Software-configured loopback mode (SPICR[0]=1, SPE=1, Master=1).
//! 3. Slave select assertion via SPISSR.
//! 4. Interrupt generation via DGIER/IPIER (DTR empty, DRR not empty) on IRQ_F2P[0] (GIC 61).
//! 5. Transmit and receive FIFO verification.
//! 6. FIFO reset verification via SPICR bits 5 and 6.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const SPI: usize = 0x41E0_0000;

// Register offsets (PG153)
const DGIER: usize = SPI + 0x1C;
const IPISR: usize = SPI + 0x20;
const IPIER: usize = SPI + 0x28;
const SRR: usize = SPI + 0x40;
const SPICR: usize = SPI + 0x60;
const SPISR: usize = SPI + 0x64;
const SPIDTR: usize = SPI + 0x68;
const SPIDRR: usize = SPI + 0x6C;
const SPISSR: usize = SPI + 0x70;

// Register bit masks
const DGIER_GIE: u32 = 1 << 31;

const INTR_DTR_EMPTY: u32 = 1 << 2;
const INTR_DRR_NOT_EMPTY: u32 = 1 << 8;

const SRR_RESET_KEY: u32 = 0x0000_000A;

const SPICR_LOOP: u32 = 1 << 0;
const SPICR_SPE: u32 = 1 << 1;
const SPICR_MASTER: u32 = 1 << 2;
const SPICR_RX_FIFO_RESET: u32 = 1 << 6;

const SPISR_RX_EMPTY: u32 = 1 << 0;
const SPISR_TX_EMPTY: u32 = 1 << 2;

const IRQ_SPI: usize = 61; // IRQ_F2P[0]

static SPI_IRQS: AtomicU32 = AtomicU32::new(0);

fn spi_isr() {
    let isr = read32(IPISR);
    if isr != 0 {
        // Toggle on write: write back to clear
        write32(IPISR, isr);
        SPI_IRQS.fetch_add(1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("axi-quad-spi demo");

    // 1. Initial / soft reset check
    let cr0 = read32(SPICR);
    let sr0 = read32(SPISR);
    println!("initial: spicr={cr0:#x} spisr={sr0:#x}");
    if cr0 != 0x0180
        || (sr0 & (SPISR_RX_EMPTY | SPISR_TX_EMPTY)) != (SPISR_RX_EMPTY | SPISR_TX_EMPTY)
    {
        println!("FAIL: bad reset values");
        rt::finish();
    }

    // Soft reset test
    write32(SPICR, 0x0);
    write32(SRR, SRR_RESET_KEY);
    let cr_reset = read32(SPICR);
    if cr_reset != 0x0180 {
        println!("FAIL: SRR soft reset did not restore SPICR");
        rt::finish();
    }
    println!("spi reset ok");

    // 2. Configure interrupts
    irq::install();
    irq::set_handler(IRQ_SPI, spi_isr);
    irq::enable_spi(IRQ_SPI);
    irq::enable();

    // Enable Global Interrupts and DRR_NOT_EMPTY / DTR_EMPTY
    write32(DGIER, DGIER_GIE);
    write32(IPIER, INTR_DRR_NOT_EMPTY | INTR_DTR_EMPTY);

    // 3. Configure Loopback mode (Loopback + SPE + Master, inhibit=0, manual_ss=0)
    write32(SPICR, SPICR_LOOP | SPICR_SPE | SPICR_MASTER);
    // Select slave 0 (active low)
    write32(SPISSR, !1);

    // 4. Data transfer in loopback mode
    let test_bytes: [u8; 4] = [0xA5, 0x5A, 0x12, 0x34];
    for &b in &test_bytes {
        write32(SPIDTR, u32::from(b));
        let sr = read32(SPISR);
        if (sr & SPISR_RX_EMPTY) != 0 {
            println!("FAIL: RX FIFO empty after transfer");
            rt::finish();
        }
        let rx = read32(SPIDRR) as u8;
        if rx != b {
            println!("FAIL: sent {b:#x}, got {rx:#x}");
            rt::finish();
        }
    }
    println!("loopback: transferred 4 bytes successfully");

    // 5. Verify interrupt occurred
    let irq_count = SPI_IRQS.load(Ordering::SeqCst);
    println!("spi irq count: {irq_count}");
    if irq_count == 0 {
        println!("FAIL: no SPI interrupt received");
        rt::finish();
    }

    // 6. Test FIFO resets via SPICR
    // Put a byte into FIFO without reading
    write32(SPIDTR, 0x77);
    if (read32(SPISR) & SPISR_RX_EMPTY) != 0 {
        println!("FAIL: RX should not be empty");
        rt::finish();
    }
    // Trigger RX FIFO reset
    let cr = read32(SPICR);
    write32(SPICR, cr | SPICR_RX_FIFO_RESET);
    let sr_after = read32(SPISR);
    if (sr_after & SPISR_RX_EMPTY) == 0 {
        println!("FAIL: RX FIFO reset failed");
        rt::finish();
    }
    println!("fifo reset ok");

    println!("RESULT: ALL PASS");
    rt::finish()
}
