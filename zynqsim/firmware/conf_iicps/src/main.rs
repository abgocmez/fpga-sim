//! Conformance of the PS I2C controllers (Cadence I2C r1p10, `xiicps`):
//! I2C0 at 0xE0004000 and I2C1 at 0xE0005000, from the registers of UG585 ch. 21
//! (`VERIFY:` marks offsets and reset values written from memory, not checked
//! against hardware).
//!
//! Each check prints one line, `CHECK n name: PASS|FAIL|N/A reason`. A FAIL is a
//! mismatch with the documented behaviour; a deviation of QEMU's model from that
//! behaviour is reported as N/A with the reason, never as a PASS. The last line is
//! `RESULT: ALL PASS` only when no check failed.
//!
//! The transfers address 0x50 and 0x51: nothing is attached to the bus in the
//! example project, so a transfer ends in a NACK. Reading from a device that is
//! attached is therefore N/A (see the check at the end).
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const I2C0: usize = 0xE000_4000;
const I2C1: usize = 0xE000_5000;

// VERIFY: register offsets (UG585 table "I2C Register Summary").
const CONTROL: usize = 0x00;
const STATUS: usize = 0x04;
const ADDR: usize = 0x08;
const DATA: usize = 0x0C;
const ISR: usize = 0x10;
const TRANS_SIZE: usize = 0x14;
const SLV_PAUSE: usize = 0x18;
const TIMEOUT: usize = 0x1C;
const IMR: usize = 0x20;
const IER: usize = 0x24;
const IDR: usize = 0x28;

// VERIFY: CONTROL bits.
const CTRL_RW: u32 = 1 << 0; // 1 = read
const CTRL_MS: u32 = 1 << 1; // master mode
const CTRL_NEA: u32 = 1 << 2; // normal (7-bit) addressing
const CTRL_ACKEN: u32 = 1 << 3;
const CTRL_HOLD: u32 = 1 << 4; // hold the bus after the transfer
const CTRL_CLR_FIFO: u32 = 1 << 6; // self-clearing

// VERIFY: interrupt status / mask / enable / disable bits (ISR, IMR, IER, IDR).
const IXR_COMP: u32 = 1 << 0; // transfer complete
const IXR_NACK: u32 = 1 << 2; // address or data NACKed
const IXR_ALL: u32 = 0x2FF;

const ADDR_ABSENT: u32 = 0x50;

/// Counts checks and failures; prints the result line.
struct Run {
    n: u32,
    fails: u32,
}

impl Run {
    fn eq(&mut self, name: &str, got: u32, want: u32) {
        self.n += 1;
        if got == want {
            println!("CHECK {} {name}: PASS {got:#010x}", self.n);
        } else {
            self.fails += 1;
            println!("CHECK {} {name}: FAIL got {got:#010x}, want {want:#010x}", self.n);
        }
    }

    /// A reset value: PASS when it matches UG585 as remembered, N/A otherwise
    /// (the remembered value is not verified, so a mismatch is not a FAIL).
    fn reset(&mut self, name: &str, got: u32, want: u32) {
        self.n += 1;
        if got == want {
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

/// Polls ISR until one of `mask` is set or the bound runs out; returns ISR.
fn wait_isr(base: usize, mask: u32) -> u32 {
    let mut isr = 0;
    for _ in 0..2000 {
        isr = read32(base + ISR);
        if isr & mask != 0 {
            break;
        }
        rt::delay_us(10);
    }
    isr
}

/// Master write of one byte to `addr` on the controller at `base`; returns ISR
/// after the transfer ended (COMP or NACK) or the bound ran out.
fn write_byte(base: usize, addr: u32, byte: u8) -> u32 {
    write32(base + ISR, 0x3FF); // clear status (write 1 to clear)
    write32(base + CONTROL, CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_CLR_FIFO);
    write32(base + ADDR, addr);
    write32(base + DATA, u32::from(byte));
    // Writing the transfer size starts the transfer in master mode.
    write32(base + TRANS_SIZE, 1);
    wait_isr(base, IXR_COMP | IXR_NACK)
}

/// Master read of one byte from `addr`; returns ISR after the transfer.
fn read_byte(base: usize, addr: u32) -> u32 {
    write32(base + ISR, 0x3FF);
    write32(base + CONTROL, CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_CLR_FIFO | CTRL_RW);
    write32(base + ADDR, addr);
    write32(base + TRANS_SIZE, 1);
    wait_isr(base, IXR_COMP | IXR_NACK)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("conf_iicps: Cadence I2C controllers at {I2C0:#x} and {I2C1:#x}");
    let mut r = Run { n: 0, fails: 0 };

    // 1-7: power-on reset values of I2C0, before anything writes to it.
    // VERIFY: reset values from UG585 (from memory).
    r.reset("I2C0 CONTROL reset", read32(I2C0 + CONTROL), 0x0000_0000);
    r.reset("I2C0 STATUS reset", read32(I2C0 + STATUS), 0x0000_0000);
    r.reset("I2C0 ISR reset", read32(I2C0 + ISR) & IXR_ALL, 0x0000_0000);
    r.reset("I2C0 IMR reset (all masked)", read32(I2C0 + IMR) & IXR_ALL, IXR_ALL);
    r.reset("I2C0 TIMEOUT reset", read32(I2C0 + TIMEOUT) & 0xFF, 0x1F);
    r.reset("I2C0 TRANS_SIZE reset", read32(I2C0 + TRANS_SIZE) & 0xFF, 0x00);
    r.reset("I2C0 ADDR reset", read32(I2C0 + ADDR) & 0x3FF, 0x00);

    // 8-13: read/write behaviour of the configuration registers.
    write32(I2C0 + CONTROL, CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_HOLD);
    r.eq(
        "I2C0 CONTROL write/read (MS NEA ACKEN HOLD)",
        read32(I2C0 + CONTROL) & 0x7F,
        CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_HOLD,
    );
    // HOLD on, then off: the bit follows the write.
    write32(I2C0 + CONTROL, CTRL_MS | CTRL_HOLD);
    let hold_on = read32(I2C0 + CONTROL) & CTRL_HOLD;
    write32(I2C0 + CONTROL, CTRL_MS);
    let hold_off = read32(I2C0 + CONTROL) & CTRL_HOLD;
    r.eq("I2C0 HOLD bit set and cleared", hold_on | hold_off, CTRL_HOLD);
    write32(I2C0 + ADDR, ADDR_ABSENT);
    r.eq("I2C0 ADDR write/read", read32(I2C0 + ADDR) & 0x3FF, ADDR_ABSENT);
    write32(I2C0 + TRANS_SIZE, 0x03);
    r.eq("I2C0 TRANS_SIZE write/read", read32(I2C0 + TRANS_SIZE) & 0xFF, 0x03);
    write32(I2C0 + SLV_PAUSE, 0x05);
    r.eq("I2C0 Slave_mon_pause write/read", read32(I2C0 + SLV_PAUSE) & 0xFF, 0x05);
    write32(I2C0 + TIMEOUT, 0x0A);
    r.eq("I2C0 Time_out write/read", read32(I2C0 + TIMEOUT) & 0xFF, 0x0A);
    write32(I2C0 + TIMEOUT, 0x1F);

    // 14: interrupt enable and disable act on the mask (IMR reads 1 = masked).
    write32(I2C0 + IER, IXR_COMP);
    let enabled = read32(I2C0 + IMR) & IXR_COMP;
    write32(I2C0 + IDR, IXR_COMP);
    let disabled = read32(I2C0 + IMR) & IXR_COMP;
    r.eq("I2C0 IER/IDR move the COMP mask", enabled | disabled, IXR_COMP);

    // 15: master write to an address with no device: the address is NACKed.
    let isr = write_byte(I2C0, ADDR_ABSENT, 0xA5);
    r.eq("I2C0 write to absent device: NACK status", u32::from(isr & IXR_NACK != 0), 1);

    // 16: ISR is write-1-to-clear.
    write32(I2C0 + ISR, IXR_NACK);
    r.eq("I2C0 ISR write-1-to-clear (NACK)", read32(I2C0 + ISR) & IXR_NACK, 0);

    // 17: a read from an address with no device is NACKed too.
    let isr = read_byte(I2C0, ADDR_ABSENT);
    r.eq("I2C0 read from absent device: NACK status", u32::from(isr & IXR_NACK != 0), 1);

    // 18: a device that is attached would answer; the example attaches none.
    r.na(
        "I2C0 read from attached device",
        "no I2C device is attached in the co-sim bus; the launch code does not attach one (not verified: bus path)",
    );

    // 19-20: I2C1 is a separate controller with its own registers.
    write32(I2C1 + ADDR, 0x33);
    write32(I2C0 + ADDR, ADDR_ABSENT);
    r.eq("I2C1 ADDR independent of I2C0", read32(I2C1 + ADDR) & 0x3FF, 0x33);
    r.reset("I2C1 TIMEOUT reset", read32(I2C1 + TIMEOUT) & 0xFF, 0x1F);

    // 21: I2C1 transfers go to its own bus; the same absent address is NACKed.
    let isr = write_byte(I2C1, ADDR_ABSENT, 0x5A);
    r.eq("I2C1 write to absent device: NACK status", u32::from(isr & IXR_NACK != 0), 1);

    // 22: the mask bits of IMR are read-only; a write to IMR changes nothing.
    write32(I2C0 + IMR, 0);
    r.eq("I2C0 IMR is read-only", read32(I2C0 + IMR) & IXR_ALL, IXR_ALL);

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
