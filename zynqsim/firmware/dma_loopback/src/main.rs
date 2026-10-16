//! AXI DMA loopback (`examples/dma-loopback`, the block design `fixtures/xsa/zs_dma_loopback.xsa`):
//! `axi_dma_0` at 0x40400000 in direct register mode, `M_AXIS_MM2S` wired to `S_AXIS_S2MM`,
//! `mm2s_introut` on IRQ_F2P[0] (GIC 61), `s2mm_introut` on IRQ_F2P[1] (GIC 62).
//!
//! The firmware copies a buffer through the DMA (MM2S reads it from DDR, the stream loops back,
//! S2MM writes it to another buffer) and checks the result against the source; both channels
//! finish with an IOC interrupt. Then it provokes the error interrupt with a zero-length
//! transfer (`DMAIntErr`), recovers with a soft reset, and copies once more.
#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const DMA: usize = 0x4040_0000;
const MM2S_DMACR: usize = DMA;
const MM2S_DMASR: usize = DMA + 0x04;
const MM2S_SA: usize = DMA + 0x18;
const MM2S_LENGTH: usize = DMA + 0x28;
const S2MM_DMACR: usize = DMA + 0x30;
const S2MM_DMASR: usize = DMA + 0x34;
const S2MM_DA: usize = DMA + 0x48;
const S2MM_LENGTH: usize = DMA + 0x58;

const CR_RS: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 2;
const CR_IOC_EN: u32 = 1 << 12;
const CR_ERR_EN: u32 = 1 << 14;
const SR_HALTED: u32 = 1 << 0;
const SR_IDLE: u32 = 1 << 1;
const SR_DMA_INT_ERR: u32 = 1 << 4;
const SR_IOC_IRQ: u32 = 1 << 12;
const SR_ERR_IRQ: u32 = 1 << 14;

const IRQ_MM2S: usize = 61; // IRQ_F2P[0]
const IRQ_S2MM: usize = 62; // IRQ_F2P[1]

const WORDS: usize = 256;
static mut SRC: [u32; WORDS] = [0; WORDS];
static mut DST: [u32; WORDS] = [0; WORDS];

static MM2S_DONE: AtomicU32 = AtomicU32::new(0);
static S2MM_DONE: AtomicU32 = AtomicU32::new(0);
static ERR_IRQS: AtomicU32 = AtomicU32::new(0);

/// Acknowledges the channel's interrupt (write 1 to clear) and counts what it was.
fn service(sr_reg: usize, done: &AtomicU32) {
    let sr = read32(sr_reg);
    if sr & SR_ERR_IRQ != 0 {
        write32(sr_reg, SR_ERR_IRQ);
        ERR_IRQS.fetch_add(1, Ordering::SeqCst);
    }
    if sr & SR_IOC_IRQ != 0 {
        write32(sr_reg, SR_IOC_IRQ);
        done.fetch_add(1, Ordering::SeqCst);
    }
}

fn mm2s_isr() {
    service(MM2S_DMASR, &MM2S_DONE);
}

fn s2mm_isr() {
    service(S2MM_DMASR, &S2MM_DONE);
}

fn wait_for(what: &AtomicU32, n: u32) -> bool {
    for _ in 0..4000 {
        if what.load(Ordering::SeqCst) >= n {
            return true;
        }
        rt::delay_us(10);
    }
    false
}

fn soft_reset() {
    write32(MM2S_DMACR, CR_RESET);
    for _ in 0..1000 {
        if read32(MM2S_DMACR) & CR_RESET == 0 {
            return;
        }
        rt::delay_us(10);
    }
}

fn copy(bytes: u32, pattern: u32) -> bool {
    let src = addr_of_mut!(SRC) as *mut u32;
    let dst = addr_of_mut!(DST) as *mut u32;
    for i in 0..WORDS {
        // SAFETY: indexes inside the two static buffers.
        unsafe {
            write_volatile(src.add(i), pattern ^ (i as u32).wrapping_mul(0x0101_0101));
            write_volatile(dst.add(i), 0);
        }
    }
    let (m0, s0) = (MM2S_DONE.load(Ordering::SeqCst), S2MM_DONE.load(Ordering::SeqCst));
    // S2MM first so it is waiting when the stream starts.
    write32(S2MM_DMACR, CR_RS | CR_IOC_EN | CR_ERR_EN);
    write32(S2MM_DA, dst as u32);
    write32(S2MM_LENGTH, bytes);
    write32(MM2S_DMACR, CR_RS | CR_IOC_EN | CR_ERR_EN);
    write32(MM2S_SA, src as u32);
    write32(MM2S_LENGTH, bytes);
    let ok = wait_for(&MM2S_DONE, m0 + 1) && wait_for(&S2MM_DONE, s0 + 1);
    if !ok {
        println!(
            "timeout: mm2s sr={:#x} s2mm sr={:#x}",
            read32(MM2S_DMASR),
            read32(S2MM_DMASR)
        );
        return false;
    }
    let mut bad = 0;
    for i in 0..(bytes as usize / 4) {
        // SAFETY: as above.
        let (a, b) = unsafe { (read_volatile(src.add(i)), read_volatile(dst.add(i))) };
        if a != b {
            bad += 1;
        }
    }
    println!(
        "copied {bytes} bytes, {bad} word(s) differ, s2mm length={}",
        read32(S2MM_LENGTH)
    );
    bad == 0 && read32(S2MM_LENGTH) == bytes
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("dma loopback: AXI DMA direct register mode");

    irq::install();
    irq::set_handler(IRQ_MM2S, mm2s_isr);
    irq::set_handler(IRQ_S2MM, s2mm_isr);
    irq::enable_spi(IRQ_MM2S);
    irq::enable_spi(IRQ_S2MM);
    irq::enable();

    soft_reset();
    println!(
        "after reset: mm2s sr={:#x} (halted={})",
        read32(MM2S_DMASR),
        read32(MM2S_DMASR) & SR_HALTED
    );

    let mut pass = copy(1024, 0x1234_5678);
    println!(
        "mm2s sr={:#x} s2mm sr={:#x} idle={}",
        read32(MM2S_DMASR),
        read32(S2MM_DMASR),
        (read32(MM2S_DMASR) & read32(S2MM_DMASR) & SR_IDLE) >> 1
    );

    // A zero-length transfer is an internal error: Err_Irq, the channel halts.
    write32(MM2S_DMACR, CR_RS | CR_ERR_EN);
    write32(MM2S_LENGTH, 0);
    let got = wait_for(&ERR_IRQS, 1);
    let sr = read32(MM2S_DMASR);
    println!(
        "zero length: err irq={got} DMAIntErr={} halted={}",
        (sr & SR_DMA_INT_ERR) >> 4,
        sr & SR_HALTED
    );
    pass &= got && sr & SR_DMA_INT_ERR != 0 && sr & SR_HALTED != 0;

    // A soft reset brings it back.
    soft_reset();
    pass &= read32(MM2S_DMASR) & (SR_DMA_INT_ERR | SR_HALTED) == SR_HALTED;
    pass &= copy(512, 0xA5A5_0000);

    if pass {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    loop {
        rt::delay_us(1000);
    }
}
