//! AXI CDMA demonstration (`examples/cdma-demo`):
//! `axi_cdma_0` at 0x43000000 on M_AXI_GP0, `cdma_introut` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests software reset, status readout, memory-to-memory DMA transfer
//! via Simple Mode, interrupt handling, and verifies destination buffer integrity.
#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const CDMA: usize = 0x4300_0000;
const CDMACR: usize = CDMA;
const CDMASR: usize = CDMA + 0x04;
const CDMA_SA: usize = CDMA + 0x18;
const CDMA_DA: usize = CDMA + 0x20;
const CDMA_BTT: usize = CDMA + 0x28;

const CR_RESET: u32 = 1 << 2;
const CR_IOC_IRQ_EN: u32 = 1 << 12;
const CR_ERR_IRQ_EN: u32 = 1 << 14;

const SR_IDLE: u32 = 1 << 1;
const SR_IOC_IRQ: u32 = 1 << 12;
const SR_ERR_IRQ: u32 = 1 << 14;

const IRQ_CDMA: usize = 61; // IRQ_F2P[0]

const WORDS: usize = 64; // 256 bytes
static mut SRC: [u32; WORDS] = [0; WORDS];
static mut DST: [u32; WORDS] = [0; WORDS];

static CDMA_DONE: AtomicU32 = AtomicU32::new(0);
static CDMA_ERRS: AtomicU32 = AtomicU32::new(0);

fn cdma_isr() {
    // Disable interrupts to immediately deassert the level-sensitive IRQ line
    write32(CDMACR, 0);
    let sr = read32(CDMASR);
    if sr & SR_ERR_IRQ != 0 {
        write32(CDMASR, SR_ERR_IRQ);
        CDMA_ERRS.fetch_add(1, Ordering::SeqCst);
    }
    if sr & SR_IOC_IRQ != 0 {
        write32(CDMASR, SR_IOC_IRQ);
        CDMA_DONE.fetch_add(1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("cdma demo: AXI CDMA 4.1 memory-to-memory DMA transfer");

    let sr_init = read32(CDMASR);
    println!("CDMA status idle: {sr_init:#010x}");
    let mut pass = (sr_init & SR_IDLE) != 0;

    // Test soft reset
    write32(CDMACR, CR_RESET);
    let cr_after_reset = read32(CDMACR);
    let sr_after_reset = read32(CDMASR);
    pass &= (cr_after_reset & CR_RESET) == 0;
    pass &= (sr_after_reset & SR_IDLE) != 0;

    // Install and enable interrupts on GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_CDMA, cdma_isr);
    irq::enable_spi(IRQ_CDMA);
    irq::enable();

    // Prepare buffers
    unsafe {
        let src_ptr = addr_of_mut!(SRC) as *mut u32;
        let dst_ptr = addr_of_mut!(DST) as *mut u32;
        for i in 0..WORDS {
            write_volatile(src_ptr.add(i), 0xC000_0000 | (i as u32));
            write_volatile(dst_ptr.add(i), 0);
        }
    }

    // Enable completion and error interrupts
    write32(CDMACR, CR_IOC_IRQ_EN | CR_ERR_IRQ_EN);

    // Set source and destination addresses
    let src_addr = addr_of_mut!(SRC) as u32;
    let dst_addr = addr_of_mut!(DST) as u32;
    write32(CDMA_SA, src_addr);
    write32(CDMA_DA, dst_addr);

    // Start Simple transfer by writing BTT (256 bytes)
    let bytes_to_transfer = (WORDS * 4) as u32;
    write32(CDMA_BTT, bytes_to_transfer);

    // Wait for completion interrupt or idle status
    let mut timeout = 100_000;
    while CDMA_DONE.load(Ordering::SeqCst) == 0 && timeout > 0 {
        timeout -= 1;
        core::hint::spin_loop();
    }

    if CDMA_DONE.load(Ordering::SeqCst) > 0 {
        println!("CDMA transfer completed ({} bytes)", bytes_to_transfer);
    } else {
        println!("CDMA transfer timed out");
        pass = false;
    }

    // Verify buffer data
    let mut buf_ok = true;
    unsafe {
        let src_ptr = addr_of_mut!(SRC) as *const u32;
        let dst_ptr = addr_of_mut!(DST) as *const u32;
        for i in 0..WORDS {
            let s = read_volatile(src_ptr.add(i));
            let d = read_volatile(dst_ptr.add(i));
            if s != d {
                buf_ok = false;
                break;
            }
        }
    }

    if buf_ok {
        println!("Buffer verification ok");
    } else {
        println!("Buffer mismatch");
        pass = false;
    }

    pass &= CDMA_ERRS.load(Ordering::SeqCst) == 0;

    if pass {
        println!("CDMA PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("CDMA FAIL");
    }

    rt::finish();
}
