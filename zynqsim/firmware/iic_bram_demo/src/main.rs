//! AXI IIC and AXI BRAM Controller demo (`examples/iic-bram-demo`, `fixtures/xsa/zs_iic_bram.xsa`):
//!
//! * `axi_bram_ctrl_0` at 0x40000000 (8 KiB): word and byte writes, read back, a word pattern
//!   over the whole window;
//! * `axi_iic_0` at 0x41600000 on Pmod JA (SCL pin 3, SDA pin 4) with a Pmod TMP2 (ADT7420, I2C
//!   0x4B): reads the ID register and the temperature through the dynamic controller, takes the
//!   receive interrupt (IRQ_F2P[0], GIC 61), and addresses a device that is not there to see the
//!   transmit-error interrupt.
//!
//! The temperature the sensor reports is the host's (`[device_values.pmod_ja] temperature`).
#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const BRAM: usize = 0x4000_0000;
const BRAM_SIZE: usize = 0x2000;

const IIC: usize = 0x4160_0000;
const GIE: usize = IIC + 0x01C;
const ISR: usize = IIC + 0x020;
const IER: usize = IIC + 0x028;
const SOFTR: usize = IIC + 0x040;
const CR: usize = IIC + 0x100;
const SR: usize = IIC + 0x104;
const TX_FIFO: usize = IIC + 0x108;
const RX_FIFO: usize = IIC + 0x10C;
const RX_FIFO_OCY: usize = IIC + 0x118;
const RX_FIFO_PIRQ: usize = IIC + 0x120;

const CR_EN: u32 = 1;
const SR_BB: u32 = 1 << 2;
const SR_RX_EMPTY: u32 = 1 << 6;
const SR_TX_EMPTY: u32 = 1 << 7;
const INT_TX_ERR: u32 = 1 << 1;
const INT_RX_FULL: u32 = 1 << 3;
const START: u32 = 1 << 8;
const STOP: u32 = 1 << 9;

const TMP2: u32 = 0x4B;
const REG_TEMP: u32 = 0x00;
const REG_ID: u32 = 0x0B;

const IRQ_IIC: usize = 61; // IRQ_F2P[0]

static RX_IRQS: AtomicU32 = AtomicU32::new(0);
static ERR_IRQS: AtomicU32 = AtomicU32::new(0);

fn iic_isr() {
    // Only what is enabled; toggle on write: writing the set bits clears them.
    let isr = read32(ISR) & read32(IER);
    write32(ISR, isr);
    if isr & INT_RX_FULL != 0 {
        RX_IRQS.fetch_add(1, Ordering::SeqCst);
        // Mask until the main program arms it again; the status bit is gone, the FIFO is
        // drained by the main program.
        write32(IER, read32(IER) & !INT_RX_FULL);
    }
    if isr & INT_TX_ERR != 0 {
        ERR_IRQS.fetch_add(1, Ordering::SeqCst);
    }
}

fn wait_tx_done() {
    // Each byte takes ~90 us of bus time at 100 kHz.
    for _ in 0..4000 {
        if read32(SR) & (SR_TX_EMPTY | SR_BB) == SR_TX_EMPTY {
            return;
        }
        rt::delay_us(10);
    }
}

/// Writes the register pointer, then reads `n` bytes with a repeated START and a STOP.
fn read_regs(addr: u32, reg: u32, n: u32) {
    write32(TX_FIFO, START | (addr << 1));
    write32(TX_FIFO, reg);
    write32(TX_FIFO, START | (addr << 1) | 1);
    write32(TX_FIFO, STOP | n);
}

fn bram_test() -> bool {
    let b = BRAM as *mut u32;
    let mut ok = true;
    write32(BRAM, 0x1122_3344);
    ok &= read32(BRAM) == 0x1122_3344;
    // Byte writes reach their lane only.
    // SAFETY: inside the BRAM window.
    unsafe {
        write_volatile((BRAM + 1) as *mut u8, 0xAA);
        write_volatile((BRAM + 6) as *mut u8, 0xBB);
    }
    ok &= read32(BRAM) == 0x1122_AA44;
    ok &= read32(BRAM + 4) == 0x00BB_0000;
    // Halfword.
    // SAFETY: as above.
    unsafe { write_volatile((BRAM + 8) as *mut u16, 0xBEEF) };
    ok &= read32(BRAM + 8) == 0x0000_BEEF;
    // The whole window.
    for i in 0..BRAM_SIZE / 4 {
        // SAFETY: as above.
        unsafe { write_volatile(b.add(i), (i as u32).wrapping_mul(0x9E37_79B1) ^ 0xC0DE) };
    }
    let mut bad = 0;
    for i in 0..BRAM_SIZE / 4 {
        // SAFETY: as above.
        let v = unsafe { read_volatile(b.add(i)) };
        if v != (i as u32).wrapping_mul(0x9E37_79B1) ^ 0xC0DE {
            bad += 1;
        }
    }
    println!("bram: {} words, {bad} differ", BRAM_SIZE / 4);
    ok && bad == 0
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("iic+bram demo");

    let bram_ok = bram_test();
    println!("bram test {}", if bram_ok { "ok" } else { "FAILED" });

    irq::install();
    irq::set_handler(IRQ_IIC, iic_isr);
    irq::enable_spi(IRQ_IIC);
    irq::enable();

    write32(SOFTR, 0xA);
    write32(CR, CR_EN);
    println!("iic: sr={:#x}", read32(SR));

    // ID register (0x0B) of the ADT7420: 0xCB.
    read_regs(TMP2, REG_ID, 1);
    wait_tx_done();
    let mut id = 0;
    if read32(SR) & SR_RX_EMPTY == 0 {
        id = read32(RX_FIFO) & 0xFF;
    }
    println!("ID {id:#04X}");

    // Temperature: two bytes, announced by the receive interrupt (RX_FIFO_PIRQ = 1: more than
    // one byte in the FIFO).
    write32(RX_FIFO_PIRQ, 1);
    write32(ISR, read32(ISR)); // clear what the ID read left behind (toggle on write)
    write32(GIE, 0x8000_0000);
    write32(IER, INT_RX_FULL | INT_TX_ERR);
    read_regs(TMP2, REG_TEMP, 2);
    let mut waited = 0;
    while RX_IRQS.load(Ordering::SeqCst) == 0 && waited < 3000 {
        rt::delay_us(10);
        waited += 1;
    }
    wait_tx_done();
    let n = read32(RX_FIFO_OCY) + 1;
    let hi = read32(RX_FIFO) & 0xFF;
    let lo = read32(RX_FIFO) & 0xFF;
    let raw = (hi << 8 | lo) as u16;
    // 13-bit mode, power-on default: the value is raw >> 3 in 1/16 degC.
    let mc = i32::from(raw as i16 >> 3) * 625 / 10;
    println!(
        "T raw={raw:#06X} mC={mc} (rx irqs={}, bytes in fifo={n})",
        RX_IRQS.load(Ordering::SeqCst)
    );

    // A device that is not on the bus: the address byte is NACKed, the core flags the error and
    // generates STOP.
    write32(TX_FIFO, START | (0x50 << 1));
    write32(TX_FIFO, 0x00);
    let mut waited = 0;
    while ERR_IRQS.load(Ordering::SeqCst) == 0 && waited < 3000 {
        rt::delay_us(10);
        waited += 1;
    }
    println!(
        "absent device: tx error irqs={} bus busy={}",
        ERR_IRQS.load(Ordering::SeqCst),
        (read32(SR) & SR_BB) >> 2
    );

    let pass = bram_ok
        && id == 0xCB
        && RX_IRQS.load(Ordering::SeqCst) == 1
        && n == 2
        && ERR_IRQS.load(Ordering::SeqCst) == 1;
    if pass {
        println!("RESULT: ALL PASS");
    } else {
        println!(
            "RESULT: FAIL (bram={bram_ok} id={id:#x} rx_irqs={} bytes={n} err_irqs={})",
            RX_IRQS.load(Ordering::SeqCst),
            ERR_IRQS.load(Ordering::SeqCst)
        );
    }
    loop {
        rt::delay_us(1000);
    }
}
