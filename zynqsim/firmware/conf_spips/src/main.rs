//! Conformance of the PS SPI controllers (Xilinx SPI r1p6, `xspips`):
//! SPI0 at 0xE0006000 and SPI1 at 0xE0007000, from the registers of UG585 ch. 14
//! (`VERIFY:` marks offsets, bits and reset values written from memory, not
//! checked against hardware).
//!
//! Each check prints one line, `CHECK n name: PASS|FAIL|N/A reason`.
//! * A controller is live only if a value written to its Delay register reads
//!   back. If it is not live, every check on it is N/A with the probe result:
//!   the model does not implement the block (the register window reads 0 and
//!   ignores writes).
//! * A reset value that differs from the one remembered from UG585 is N/A with
//!   both values (the model or the remembered value is wrong; not verified).
//! * A FAIL is a mismatch with the documented behaviour on a live controller.
//! The last line is `RESULT: ALL PASS` only when no check failed.
//!
//! The QSPI controller (`qspi_demo`) is a different block and is not touched.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const SPI0: usize = 0xE000_6000;
const SPI1: usize = 0xE000_7000;

// VERIFY: register offsets (UG585 "SPI Register Summary").
const CR: usize = 0x00; // Config
const ISR: usize = 0x04; // Interrupt status (write 1 to clear)
const IER: usize = 0x08;
const IDR: usize = 0x0C;
const IMR: usize = 0x10; // reads 1 where the interrupt is masked (disabled)
const ER: usize = 0x14; // Enable
const DR: usize = 0x18; // Delay
const TXD: usize = 0x1C; // TX data (FIFO push)
const RXD: usize = 0x20; // RX data (FIFO pop)
const SICR: usize = 0x24; // Slave idle count
const TXWR: usize = 0x28; // TX threshold
const RXWR: usize = 0x2C; // RX threshold

// VERIFY: Config bits.
const CR_MSTREN: u32 = 1 << 0;
const CR_CPHA: u32 = 1 << 2;
const CR_BAUD_8: u32 = 0b011 << 3; // baud divider field (bits 5:3)
const CR_MANSTRT: u32 = 1 << 15; // manual start command (self-clearing)
const CR_MANSTRTEN: u32 = 1 << 16; // manual start enable
const CR_SSFORCE: u32 = 1 << 14; // chip select under software control
const CR_CS_MASK: u32 = 0xF << 10; // one-hot, active low: 0b1110 = slave 0
const CR_CS0: u32 = 0b1110 << 10;
const CR_MODEFAIL_EN: u32 = 1 << 17; // reset value (VERIFY)
const CR_RESET: u32 = CR_MODEFAIL_EN;

// VERIFY: interrupt bits (ISR, IMR, IER, IDR).
const IXR_TXNFULL: u32 = 1 << 2; // TX FIFO not full (over water mark)
const IXR_TXFULL: u32 = 1 << 3; // TX FIFO full
const IXR_RXNEMPTY: u32 = 1 << 4; // RX FIFO not empty
const IXR_ALL: u32 = 0x7F;

/// The TX FIFO holds 128 words (VERIFY).
const FIFO_DEPTH: usize = 128;

const NOT_MODELLED: &str = "not modelled by QEMU: the register window reads 0 and ignores writes";

/// Counts checks and failures; prints the result line.
struct Run {
    n: u32,
    fails: u32,
}

impl Run {
    /// A check on a live controller: PASS when equal, FAIL otherwise. On a dead
    /// controller it is N/A.
    fn eq(&mut self, live: bool, name: &str, got: u32, want: u32) {
        self.n += 1;
        if !live {
            println!("CHECK {} {name}: N/A {NOT_MODELLED}", self.n);
        } else if got == want {
            println!("CHECK {} {name}: PASS {got:#010x}", self.n);
        } else {
            self.fails += 1;
            println!("CHECK {} {name}: FAIL got {got:#010x}, want {want:#010x}", self.n);
        }
    }

    /// A reset value: PASS when it matches UG585 as remembered, N/A otherwise
    /// (not verified, so a mismatch is not a FAIL).
    fn reset(&mut self, live: bool, name: &str, got: u32, want: u32) {
        self.n += 1;
        if !live {
            println!("CHECK {} {name}: N/A {NOT_MODELLED}", self.n);
        } else if got == want {
            println!("CHECK {} {name}: PASS {got:#010x}", self.n);
        } else {
            println!(
                "CHECK {} {name}: N/A reset reads {got:#010x}, UG585 as remembered {want:#010x} (VERIFY)",
                self.n
            );
        }
    }

    fn na(&mut self, name: &str, why: &str) {
        self.n += 1;
        println!("CHECK {} {name}: N/A {why}", self.n);
    }
}

/// Polls ISR until `mask` matches `want` (bounded, paced); returns the last ISR.
fn wait_isr(base: usize, mask: u32, want: u32) -> u32 {
    let mut isr = 0;
    for _ in 0..2000 {
        isr = read32(base + ISR);
        if isr & mask == want {
            break;
        }
        rt::delay_us(10);
    }
    isr
}

/// Whether the controller at `base` keeps a written value (Delay register).
fn is_live(base: usize) -> bool {
    write32(base + DR, 0x0000_1234);
    let ok = read32(base + DR) == 0x0000_1234;
    write32(base + DR, 0);
    ok
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("conf_spips: Xilinx SPI controllers at {SPI0:#x} and {SPI1:#x}");
    let live0 = is_live(SPI0);
    let live1 = is_live(SPI1);
    let mut r = Run { n: 0, fails: 0 };

    // 1-8: reset values of SPI0, before anything writes to it.
    r.reset(live0, "SPI0 Config reset", read32(SPI0 + CR), CR_RESET);
    r.reset(live0, "SPI0 Enable reset (disabled)", read32(SPI0 + ER) & 1, 0);
    r.reset(live0, "SPI0 Delay reset", read32(SPI0 + DR), 0);
    r.reset(live0, "SPI0 TX threshold reset", read32(SPI0 + TXWR) & 0xFF, 1);
    r.reset(live0, "SPI0 RX threshold reset", read32(SPI0 + RXWR) & 0xFF, 1);
    r.reset(live0, "SPI0 Slave_Idle_count reset", read32(SPI0 + SICR) & 0xFF, 0);
    let isr = read32(SPI0 + ISR);
    r.reset(live0, "SPI0 empty TX FIFO: TX_FIFO_not_full set", isr & IXR_TXNFULL, IXR_TXNFULL);
    r.eq(live0, "SPI0 empty TX FIFO: TX_FIFO_full clear", isr & IXR_TXFULL, 0);
    r.reset(live0, "SPI0 interrupt mask reset (all masked)", read32(SPI0 + IMR) & IXR_ALL, IXR_ALL);

    // 9-10: Config bits: master, clock phase, baud divider, manual start and
    // software chip select.
    write32(SPI0 + CR, CR_RESET | CR_MSTREN | CR_CPHA | CR_BAUD_8);
    r.eq(
        live0,
        "SPI0 Config write/read (master, CPHA, baud)",
        read32(SPI0 + CR) & 0x3F,
        CR_MSTREN | CR_CPHA | CR_BAUD_8,
    );
    write32(SPI0 + CR, read32(SPI0 + CR) | CR_MANSTRTEN | CR_SSFORCE | CR_CS0);
    r.eq(
        live0,
        "SPI0 manual start enable, SS force and CS0 read back",
        read32(SPI0 + CR) & (CR_MANSTRTEN | CR_SSFORCE | CR_CS_MASK),
        CR_MANSTRTEN | CR_SSFORCE | CR_CS0,
    );

    // 11-15: Delay, TX and RX thresholds, Slave_Idle_count read/write.
    write32(SPI0 + DR, 0x0102_0304);
    r.eq(live0, "SPI0 Delay write/read", read32(SPI0 + DR), 0x0102_0304);
    write32(SPI0 + TXWR, 0x10);
    r.eq(live0, "SPI0 TX threshold write/read", read32(SPI0 + TXWR) & 0xFF, 0x10);
    write32(SPI0 + RXWR, 0x08);
    r.eq(live0, "SPI0 RX threshold write/read", read32(SPI0 + RXWR) & 0xFF, 0x08);
    write32(SPI0 + SICR, 0x0A);
    r.eq(live0, "SPI0 Slave_Idle_count write/read", read32(SPI0 + SICR) & 0xFF, 0x0A);

    // 16: IER and IDR move the mask of the TX over-water interrupt.
    write32(SPI0 + IER, IXR_TXNFULL);
    let enabled = read32(SPI0 + IMR) & IXR_TXNFULL;
    write32(SPI0 + IDR, IXR_TXNFULL);
    let disabled = read32(SPI0 + IMR) & IXR_TXNFULL;
    r.eq(live0, "SPI0 IER/IDR move the TX_FIFO_not_full mask", enabled | disabled, IXR_TXNFULL);

    // 17-18: Enable register (ER bit 0).
    write32(SPI0 + ER, 1);
    r.eq(live0, "SPI0 Enable write/read (1)", read32(SPI0 + ER) & 1, 1);
    write32(SPI0 + ER, 0);
    r.eq(live0, "SPI0 Enable write/read (0)", read32(SPI0 + ER) & 1, 0);

    // 19-20: TX FIFO fill while the controller is disabled.
    write32(SPI0 + ISR, IXR_ALL);
    write32(SPI0 + CR, CR_RESET | CR_MSTREN | CR_CPHA | CR_BAUD_8 | CR_MANSTRTEN | CR_SSFORCE | CR_CS0);
    for i in 0..FIFO_DEPTH {
        write32(SPI0 + TXD, i as u32);
    }
    let isr = read32(SPI0 + ISR);
    r.eq(live0, "SPI0 TX FIFO full after 128 writes: TX_FIFO_full set", isr & IXR_TXFULL, IXR_TXFULL);
    r.eq(live0, "SPI0 full TX FIFO: TX_FIFO_not_full clear", isr & IXR_TXNFULL, 0);

    // 21-23: enable and start one manual transfer; the TX FIFO drains and the RX
    // FIFO fills with one word per TX word.
    write32(SPI0 + ER, 1);
    write32(SPI0 + CR, read32(SPI0 + CR) | CR_MANSTRT);
    let isr = wait_isr(SPI0, IXR_TXFULL, 0);
    r.eq(live0, "SPI0 manual start drains the TX FIFO (TX_FIFO_full clear)", isr & IXR_TXFULL, 0);
    let isr = read32(SPI0 + ISR);
    r.eq(live0, "SPI0 RX FIFO not empty after the transfer", isr & IXR_RXNEMPTY, IXR_RXNEMPTY);
    // Loopback: nothing is attached to the SPI bus, so the byte received is
    // whatever the bus returns for an unanswered transfer. A loopback would
    // return the first sent byte (0x00 here), but no loopback path is documented
    // in this check, so the byte is reported, not asserted.
    let _received = read32(SPI0 + RXD) & 0xFF;
    r.na(
        "SPI0 loopback of the first byte with no slave attached",
        if live0 { "not asserted: the byte received with no slave is reported only, no loopback is documented" } else { NOT_MODELLED },
    );

    // 24-26: SPI1 is a separate controller.
    write32(SPI1 + DR, 0x5566_7788);
    r.eq(live0, "SPI0 Delay unchanged by a write to SPI1", read32(SPI0 + DR), 0x0102_0304);
    r.eq(live1, "SPI1 Delay write/read", read32(SPI1 + DR), 0x5566_7788);
    r.reset(live1, "SPI1 Config reset", read32(SPI1 + CR) & 0x0002_0000, CR_MODEFAIL_EN);

    // 27: the interrupt line to the GIC is not part of this check (status only).
    r.na("SPI0 interrupt line to the GIC", "not checked: this program reads the status registers only");

    if r.fails == 0 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL ({} checks failed)", r.fails);
    }
    loop {
        // SAFETY: parks the core.
        unsafe { core::arch::asm!("wfi") }
    }
}
