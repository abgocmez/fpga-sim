//! AXI VDMA demonstration (`examples/vdma-demo`):
//! `axi_vdma_0` at 0x43000000, `M_AXIS_MM2S` looped back to `S_AXIS_S2MM`,
//! `mm2s_introut` on IRQ_F2P[0] (GIC 61), `s2mm_introut` on IRQ_F2P[1] (GIC 62).
//!
//! The firmware tests 2D video frame transfers in park and circular modes,
//! verifies memory integrity between source and destination frames, and checks
//! completion interrupts.
#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const VDMA: usize = 0x4300_0000;
const MM2S_VDMACR: usize = VDMA;
const MM2S_VDMASR: usize = VDMA + 0x04;
const PARK_PTR_REG: usize = VDMA + 0x28;
const VDMA_VERSION: usize = VDMA + 0x2C;
const S2MM_VDMACR: usize = VDMA + 0x30;
const S2MM_VDMASR: usize = VDMA + 0x34;

const MM2S_VSIZE: usize = VDMA + 0x50;
const MM2S_HSIZE: usize = VDMA + 0x54;
const MM2S_FRMDLY_STRIDE: usize = VDMA + 0x58;
const MM2S_START_ADDRESS_BASE: usize = VDMA + 0x5C;

const S2MM_VSIZE: usize = VDMA + 0xA0;
const S2MM_HSIZE: usize = VDMA + 0xA4;
const S2MM_FRMDLY_STRIDE: usize = VDMA + 0xA8;
const S2MM_START_ADDRESS_BASE: usize = VDMA + 0xAC;

const CR_RS: u32 = 1 << 0;
const CR_CIRCULAR_PARK: u32 = 1 << 1;
const CR_RESET: u32 = 1 << 2;
const CR_IOC_IRQ_EN: u32 = 1 << 12;
const CR_ERR_IRQ_EN: u32 = 1 << 14;

const SR_IOC_IRQ: u32 = 1 << 12;
const SR_ERR_IRQ: u32 = 1 << 14;

const IRQ_MM2S: usize = 61; // IRQ_F2P[0]
const IRQ_S2MM: usize = 62; // IRQ_F2P[1]

const LINES: usize = 8;
const BYTES_PER_LINE: usize = 32;
const TOTAL_BYTES: usize = LINES * BYTES_PER_LINE;
const WORDS: usize = TOTAL_BYTES / 4;

static mut SRC: [u32; WORDS] = [0; WORDS];
static mut DST: [u32; WORDS] = [0; WORDS];

static MM2S_DONE: AtomicU32 = AtomicU32::new(0);
static S2MM_DONE: AtomicU32 = AtomicU32::new(0);

fn service(sr_reg: usize, done: &AtomicU32) {
    let sr = read32(sr_reg);
    if sr & SR_ERR_IRQ != 0 {
        write32(sr_reg, SR_ERR_IRQ);
    }
    if sr & SR_IOC_IRQ != 0 {
        write32(sr_reg, SR_IOC_IRQ);
        done.fetch_add(1, Ordering::SeqCst);
    }
}

fn mm2s_isr() {
    service(MM2S_VDMASR, &MM2S_DONE);
}

fn s2mm_isr() {
    service(S2MM_VDMASR, &S2MM_DONE);
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
    write32(MM2S_VDMACR, CR_RESET);
    write32(S2MM_VDMACR, CR_RESET);
    for _ in 0..1000 {
        if (read32(MM2S_VDMACR) & CR_RESET == 0) && (read32(S2MM_VDMACR) & CR_RESET == 0) {
            return;
        }
        rt::delay_us(10);
    }
}

fn test_park_mode() -> bool {
    let src = addr_of_mut!(SRC) as *mut u32;
    let dst = addr_of_mut!(DST) as *mut u32;

    for i in 0..WORDS {
        unsafe {
            write_volatile(
                src.add(i),
                0xCAFE_0000 ^ (i as u32).wrapping_mul(0x0101_0101),
            );
            write_volatile(dst.add(i), 0);
        }
    }

    let (m0, s0) = (
        MM2S_DONE.load(Ordering::SeqCst),
        S2MM_DONE.load(Ordering::SeqCst),
    );

    // Park pointer reg: RD=0, WR=0
    write32(PARK_PTR_REG, 0);

    // Configure S2MM
    write32(S2MM_START_ADDRESS_BASE, dst as u32);
    write32(S2MM_FRMDLY_STRIDE, BYTES_PER_LINE as u32);
    write32(S2MM_HSIZE, BYTES_PER_LINE as u32);
    write32(S2MM_VDMACR, CR_RS | CR_IOC_IRQ_EN | CR_ERR_IRQ_EN);
    write32(S2MM_VSIZE, LINES as u32);

    // Configure MM2S
    write32(MM2S_START_ADDRESS_BASE, src as u32);
    write32(MM2S_FRMDLY_STRIDE, BYTES_PER_LINE as u32);
    write32(MM2S_HSIZE, BYTES_PER_LINE as u32);
    write32(MM2S_VDMACR, CR_RS | CR_IOC_IRQ_EN | CR_ERR_IRQ_EN);
    write32(MM2S_VSIZE, LINES as u32); // Triggers transfer

    let ok = wait_for(&MM2S_DONE, m0 + 1) && wait_for(&S2MM_DONE, s0 + 1);
    if !ok {
        println!(
            "park mode timeout: mm2s sr={:#x} s2mm sr={:#x}",
            read32(MM2S_VDMASR),
            read32(S2MM_VDMASR)
        );
        return false;
    }

    let mut bad = 0;
    for i in 0..WORDS {
        let (a, b) = unsafe { (read_volatile(src.add(i)), read_volatile(dst.add(i))) };
        if a != b {
            bad += 1;
        }
    }

    println!("park mode copied {TOTAL_BYTES} bytes, {bad} words differ");
    bad == 0
}

fn test_circular_mode() -> bool {
    let src = addr_of_mut!(SRC) as *mut u32;
    let dst = addr_of_mut!(DST) as *mut u32;

    for i in 0..WORDS {
        unsafe {
            write_volatile(
                src.add(i),
                0xFACE_0000 ^ (i as u32).wrapping_mul(0x1010_1010),
            );
            write_volatile(dst.add(i), 0);
        }
    }

    let (m0, s0) = (
        MM2S_DONE.load(Ordering::SeqCst),
        S2MM_DONE.load(Ordering::SeqCst),
    );

    // Circular mode: CR_CIRCULAR_PARK = 1
    write32(S2MM_START_ADDRESS_BASE, dst as u32);
    write32(S2MM_FRMDLY_STRIDE, BYTES_PER_LINE as u32);
    write32(S2MM_HSIZE, BYTES_PER_LINE as u32);
    write32(
        S2MM_VDMACR,
        CR_RS | CR_CIRCULAR_PARK | CR_IOC_IRQ_EN | CR_ERR_IRQ_EN,
    );
    write32(S2MM_VSIZE, LINES as u32);

    write32(MM2S_START_ADDRESS_BASE, src as u32);
    write32(MM2S_FRMDLY_STRIDE, BYTES_PER_LINE as u32);
    write32(MM2S_HSIZE, BYTES_PER_LINE as u32);
    write32(
        MM2S_VDMACR,
        CR_RS | CR_CIRCULAR_PARK | CR_IOC_IRQ_EN | CR_ERR_IRQ_EN,
    );
    write32(MM2S_VSIZE, LINES as u32);

    let ok = wait_for(&MM2S_DONE, m0 + 1) && wait_for(&S2MM_DONE, s0 + 1);
    if !ok {
        println!("circular mode timeout");
        return false;
    }

    let mut bad = 0;
    for i in 0..WORDS {
        let (a, b) = unsafe { (read_volatile(src.add(i)), read_volatile(dst.add(i))) };
        if a != b {
            bad += 1;
        }
    }

    println!("circular mode copied {TOTAL_BYTES} bytes, {bad} words differ");
    bad == 0
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vdma demo: AXI VDMA 6.3 2D frame transfer");

    let ver = read32(VDMA_VERSION);
    println!("VDMA core version: {ver:#x}");

    irq::install();
    irq::set_handler(IRQ_MM2S, mm2s_isr);
    irq::set_handler(IRQ_S2MM, s2mm_isr);
    irq::enable_spi(IRQ_MM2S);
    irq::enable_spi(IRQ_S2MM);
    irq::enable();

    soft_reset();
    println!(
        "after reset: mm2s sr={:#x} s2mm sr={:#x}",
        read32(MM2S_VDMASR),
        read32(S2MM_VDMASR)
    );

    let mut pass = ver == 0x0603_0000;
    pass &= test_park_mode();
    pass &= test_circular_mode();

    if pass {
        println!("VDMA PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
