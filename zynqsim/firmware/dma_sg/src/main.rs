//! AXI DMA in scatter-gather mode (`examples/dma-sg`): a 64-byte packet is sent as two MM2S
//! buffer descriptors (SOF on the first, EOF on the second), loops back on the AXI4-Stream and
//! is received into two S2MM descriptors of 48 bytes each (the second ends at TLAST after 16
//! bytes). Each channel interrupts once, after both of its descriptors (IRQThreshold = 2); the
//! firmware then checks the data and the descriptors' status words.
#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const DMA: usize = 0x4040_0000;
const MM2S_DMACR: usize = DMA;
const MM2S_DMASR: usize = DMA + 0x04;
const MM2S_CURDESC: usize = DMA + 0x08;
const MM2S_TAILDESC: usize = DMA + 0x10;
const S2MM_DMACR: usize = DMA + 0x30;
const S2MM_DMASR: usize = DMA + 0x34;
const S2MM_CURDESC: usize = DMA + 0x38;
const S2MM_TAILDESC: usize = DMA + 0x40;

const CR_RS: u32 = 1 << 0;
const CR_RESET: u32 = 1 << 2;
const CR_IOC_EN: u32 = 1 << 12;
const CR_ERR_EN: u32 = 1 << 14;
const SR_SG_INCLD: u32 = 1 << 3;
const SR_IDLE: u32 = 1 << 1;
const SR_IOC_IRQ: u32 = 1 << 12;
const SR_ERR_IRQ: u32 = 1 << 14;

const BD_SOF: u32 = 1 << 27;
const BD_EOF: u32 = 1 << 26;
const BD_CMPLT: u32 = 1 << 31;
const BD_RX_SOF: u32 = 1 << 27;
const BD_RX_EOF: u32 = 1 << 26;

const IRQ_MM2S: usize = 61;
const IRQ_S2MM: usize = 62;

/// A buffer descriptor (64-byte aligned).
#[repr(C, align(64))]
struct Bd {
    next: u32,
    next_msb: u32,
    buf: u32,
    buf_msb: u32,
    _reserved: [u32; 2],
    control: u32,
    status: u32,
    _app: [u32; 8],
}

const BD0: Bd = Bd {
    next: 0,
    next_msb: 0,
    buf: 0,
    buf_msb: 0,
    _reserved: [0; 2],
    control: 0,
    status: 0,
    _app: [0; 8],
};

static mut TX: [Bd; 2] = [BD0, BD0];
static mut RX: [Bd; 2] = [BD0, BD0];
static mut SRC: [u8; 64] = [0; 64];
static mut DST: [u8; 96] = [0; 96];

static MM2S_IRQS: AtomicU32 = AtomicU32::new(0);
static S2MM_IRQS: AtomicU32 = AtomicU32::new(0);
static ERRORS: AtomicU32 = AtomicU32::new(0);

fn service(sr_reg: usize, n: &AtomicU32) {
    let sr = read32(sr_reg);
    if sr & SR_ERR_IRQ != 0 {
        write32(sr_reg, SR_ERR_IRQ);
        ERRORS.fetch_add(1, Ordering::SeqCst);
    }
    if sr & SR_IOC_IRQ != 0 {
        write32(sr_reg, SR_IOC_IRQ);
        n.fetch_add(1, Ordering::SeqCst);
    }
}

fn mm2s_isr() {
    service(MM2S_DMASR, &MM2S_IRQS);
}

fn s2mm_isr() {
    service(S2MM_DMASR, &S2MM_IRQS);
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

fn status(bd: *const Bd) -> u32 {
    // SAFETY: a descriptor in a static array.
    unsafe { read_volatile(core::ptr::addr_of!((*bd).status)) }
}

fn set_bd(bd: *mut Bd, next: u32, buf: u32, control: u32) {
    // SAFETY: a descriptor in a static array.
    unsafe {
        write_volatile(core::ptr::addr_of_mut!((*bd).next), next);
        write_volatile(core::ptr::addr_of_mut!((*bd).buf), buf);
        write_volatile(core::ptr::addr_of_mut!((*bd).control), control);
        write_volatile(core::ptr::addr_of_mut!((*bd).status), 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("dma sg: AXI DMA scatter-gather mode");

    irq::install();
    irq::set_handler(IRQ_MM2S, mm2s_isr);
    irq::set_handler(IRQ_S2MM, s2mm_isr);
    irq::enable_spi(IRQ_MM2S);
    irq::enable_spi(IRQ_S2MM);
    irq::enable();

    write32(MM2S_DMACR, CR_RESET);
    rt::delay_us(10);
    println!(
        "sr after reset: {:#x} (SGIncld={})",
        read32(MM2S_DMASR),
        (read32(MM2S_DMASR) & SR_SG_INCLD) >> 3
    );

    let src = addr_of_mut!(SRC) as *mut u8;
    let dst = addr_of_mut!(DST) as *mut u8;
    for i in 0..64 {
        // SAFETY: inside SRC.
        unsafe { write_volatile(src.add(i), (i as u8).wrapping_mul(3).wrapping_add(7)) };
    }

    let tx = addr_of_mut!(TX) as *mut Bd;
    let rx = addr_of_mut!(RX) as *mut Bd;
    // SAFETY: element addresses of the static arrays.
    let (tx0, tx1, rx0, rx1) = unsafe { (tx, tx.add(1), rx, rx.add(1)) };
    set_bd(tx0, tx1 as u32, src as u32, BD_SOF | 40);
    set_bd(tx1, tx0 as u32, src as u32 + 40, BD_EOF | 24);
    set_bd(rx0, rx1 as u32, dst as u32, 48);
    set_bd(rx1, rx0 as u32, dst as u32 + 48, 48);

    // Receive first: CURDESC, run, then the tail pointer starts the engine.
    write32(S2MM_CURDESC, rx0 as u32);
    write32(S2MM_DMACR, CR_RS | CR_IOC_EN | CR_ERR_EN | (2 << 16));
    write32(S2MM_TAILDESC, rx1 as u32);
    write32(MM2S_CURDESC, tx0 as u32);
    write32(MM2S_DMACR, CR_RS | CR_IOC_EN | CR_ERR_EN | (2 << 16));
    write32(MM2S_TAILDESC, tx1 as u32);

    let ok = wait_for(&MM2S_IRQS, 1) && wait_for(&S2MM_IRQS, 1);
    println!(
        "irqs: mm2s={} s2mm={} errors={}",
        MM2S_IRQS.load(Ordering::SeqCst),
        S2MM_IRQS.load(Ordering::SeqCst),
        ERRORS.load(Ordering::SeqCst)
    );
    println!(
        "tx status: {:#x} {:#x}",
        status(tx0),
        status(tx1)
    );
    println!(
        "rx status: {:#x} {:#x}",
        status(rx0),
        status(rx1)
    );
    let mut bad = 0;
    for i in 0..64 {
        // SAFETY: inside SRC/DST.
        let (a, b) = unsafe { (read_volatile(src.add(i)), read_volatile(dst.add(i))) };
        if a != b {
            bad += 1;
        }
    }
    println!(
        "data: {bad} byte(s) differ; idle mm2s={} s2mm={}",
        (read32(MM2S_DMASR) & SR_IDLE) >> 1,
        (read32(S2MM_DMASR) & SR_IDLE) >> 1
    );

    let pass = ok
        && bad == 0
        && ERRORS.load(Ordering::SeqCst) == 0
        && status(tx0) == (BD_CMPLT | 40)
        && status(tx1) == (BD_CMPLT | 24)
        && status(rx0) == (BD_CMPLT | BD_RX_SOF | 48)
        && status(rx1) == (BD_CMPLT | BD_RX_EOF | 16);
    if pass {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    loop {
        rt::delay_us(1000);
    }
}
