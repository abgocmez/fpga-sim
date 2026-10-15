//! QSPI flash demo for the Zybo Z7-20 (PS QSPI controller, S25FL128S).
//!
//! 1. Reads the JEDEC ID with the `RDID` (0x9F) command through the
//!    controller's TX and RX FIFOs.
//! 2. Reads 64 bytes from a flash offset the same way (`READ`, 0x03).
//! 3. Switches the controller to linear (memory-mapped) mode and reads the
//!    same 64 bytes at `0xFC000000 + offset`.
//! 4. Checks that both reads agree and that the ID looks like a 16 MiB part.
//!
//! The host puts known data into the flash image (`zynqsim qspi-image`), so
//! the test can compare what appears on the UART with it.
//!
//! VERIFY: register offsets and bit positions are from memory of the Zynq TRM
//! (UG585, ch. 12) and the standalone `xqspips` driver; they are exercised
//! against QEMU's controller model, not checked on hardware. Quad/dual modes,
//! DMA, interrupts and the dual-flash arrangement are not implemented.
#![no_std]
#![no_main]

use rt::{print, println, read32, write32};

const QSPI: usize = 0xE000_D000;
/// Start of the linear (memory-mapped) read window.
const LINEAR: usize = 0xFC00_0000;

const CR: usize = QSPI;
const SR: usize = QSPI + 0x04;
const ER: usize = QSPI + 0x14;
const TXD0: usize = QSPI + 0x1C; // 4 bytes
const TXD1: usize = QSPI + 0x80; // 1 byte
const TXD2: usize = QSPI + 0x84; // 2 bytes
const TXD3: usize = QSPI + 0x88; // 3 bytes
const RXD: usize = QSPI + 0x20;
const TXWR: usize = QSPI + 0x28;
const RXWR: usize = QSPI + 0x2C;
const LQSPI_CR: usize = QSPI + 0xA0;

const CR_IFMODE: u32 = 1 << 31; // flash memory interface mode
const CR_SS_FORCE: u32 = 1 << 14; // chip select under software control
const CR_SS_SHIFT: u32 = 10;
const CR_SS_MASK: u32 = 0xF << CR_SS_SHIFT;
const CR_FIFO_32: u32 = 3 << 6;
const CR_MASTER: u32 = 1;

const SR_RX_NOT_EMPTY: u32 = 1 << 4;

const LQSPI_LINEAR: u32 = 1 << 31;
const LQSPI_READ: u32 = 0x03; // instruction code: READ, no dummy bytes

/// Flash offset the host image carries its test data at.
const DATA_OFFSET: u32 = 0x0010_0000;
const DATA_LEN: usize = 64;

fn fail(why: &str) -> ! {
    println!("RESULT: FAIL: {why}");
    loop {
        // SAFETY: parks the core; nothing else is left to do.
        unsafe { core::arch::asm!("wfi") }
    }
}

/// Polls `done` a bounded number of times, paced.
fn wait(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..4000 {
        if done() {
            return;
        }
        rt::delay_us(5);
    }
    println!("timeout waiting for {what}");
    fail("controller timeout");
}

fn set_cs(selected: bool) {
    // Active-low one-hot: 0b1110 selects chip select 0, 0b1111 none.
    let ss = if selected { 0b1110 } else { 0b1111 };
    write32(CR, (read32(CR) & !CR_SS_MASK) | (ss << CR_SS_SHIFT));
}

fn init() {
    // Leave linear mode, then configure the controller as a master with the
    // chip select under software control, and enable it.
    write32(ER, 0);
    write32(LQSPI_CR, 0);
    write32(
        CR,
        CR_IFMODE | CR_SS_FORCE | CR_FIFO_32 | CR_SS_MASK | (0b010 << 3) | CR_MASTER | (1 << 17),
    );
    write32(TXWR, 1);
    write32(RXWR, 1);
    write32(ER, 1);
    // Drain anything left in the RX FIFO.
    for _ in 0..64 {
        if read32(SR) & SR_RX_NOT_EMPTY == 0 {
            break;
        }
        let _ = read32(RXD);
    }
}

/// One chip-select-framed transfer of `tx.len()` bytes (at most 128).
/// `rx[i]` is the byte received while `tx[i]` was sent.
fn transfer(tx: &[u8], rx: &mut [u8]) {
    assert!(tx.len() == rx.len() && tx.len() <= 128);
    set_cs(true);
    // The controller sends each TX word least significant byte first. A full
    // word goes to TXD0; a trailing 1, 2 or 3 bytes go to TXD1, TXD2, TXD3.
    for part in tx.chunks(4) {
        let mut w = [0u8; 4];
        w[..part.len()].copy_from_slice(part);
        let reg = match part.len() {
            4 => TXD0,
            1 => TXD1,
            2 => TXD2,
            _ => TXD3,
        };
        write32(reg, u32::from_le_bytes(w));
    }
    // Every TX word produces one RX word. A full word carries its four bytes
    // in order; a 1-3 byte word carries them in the UPPER bytes (the Linux
    // zynq-qspi driver reads `((u8 *)&data) + 4 - size`).
    for part in rx.chunks_mut(4) {
        wait("a received word", || read32(SR) & SR_RX_NOT_EMPTY != 0);
        let w = read32(RXD).to_le_bytes();
        part.copy_from_slice(&w[4 - part.len()..]);
    }
    set_cs(false);
}

/// Waits until the flash's write-in-progress bit (RDSR bit 0) is clear.
fn wait_idle() {
    for _ in 0..2000 {
        let mut rx = [0u8; 2];
        transfer(&[0x05, 0], &mut rx);
        if rx[1] & 1 == 0 {
            return;
        }
        rt::delay_us(10);
    }
    fail("the flash stays busy (WIP never clears)");
}

/// Write enable.
fn write_enable() {
    let mut rx = [0u8; 1];
    transfer(&[0x06], &mut rx);
    let mut sr = [0u8; 2];
    transfer(&[0x05, 0], &mut sr);
    if sr[1] & 2 == 0 {
        println!("RDSR after WREN: {:02x} {:02x}", sr[0], sr[1]);
        fail("WREN did not set the write-enable latch");
    }
}

fn read_flash(offset: u32, out: &mut [u8]) {
    let mut tx = [0u8; 4 + 64];
    let mut rx = [0u8; 4 + 64];
    assert!(out.len() <= 64);
    tx[0] = 0x03;
    tx[1] = (offset >> 16) as u8;
    tx[2] = (offset >> 8) as u8;
    tx[3] = offset as u8;
    let n = 4 + out.len();
    transfer(&tx[..n], &mut rx[..n]);
    out.copy_from_slice(&rx[4..n]);
}

fn hex(bytes: &[u8]) {
    for b in bytes {
        const D: &[u8; 16] = b"0123456789abcdef";
        rt::putc(D[usize::from(b >> 4)]);
        rt::putc(D[usize::from(b & 15)]);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("qspi demo: Zynq QSPI controller at {QSPI:#x}");
    init();

    // RDID: command byte, then the ID bytes; byte 0 of the answer is junk.
    let mut tx = [0u8; 8];
    tx[0] = 0x9F;
    let mut rx = [0u8; 8];
    transfer(&tx, &mut rx);
    let id = &rx[1..];
    print_id(id);
    let (manufacturer, mem_type, capacity) = (id[0], id[1], id[2]);
    println!(
        "JEDEC: manufacturer {manufacturer:#04x} type {mem_type:#04x} capacity code {capacity:#04x} = {} bytes",
        1u32.checked_shl(u32::from(capacity)).unwrap_or(0)
    );
    if manufacturer == 0 || manufacturer == 0xFF {
        fail("no flash answers RDID");
    }
    if capacity != 0x18 {
        fail("capacity code is not 0x18 (16 MiB)");
    }

    // READ 0x03 + 24-bit address, then dummy words to clock the data out.
    let mut cmd = [0u8; 4 + DATA_LEN];
    cmd[0] = 0x03;
    cmd[1] = (DATA_OFFSET >> 16) as u8;
    cmd[2] = (DATA_OFFSET >> 8) as u8;
    cmd[3] = DATA_OFFSET as u8;
    let mut got = [0u8; 4 + DATA_LEN];
    transfer(&cmd, &mut got);
    let fifo = &got[4..];
    print!("FIFO read @{DATA_OFFSET:#010x}: ");
    hex(fifo);
    println!();

    // Linear mode: the controller issues READ itself when the CPU reads the
    // window. As the FSBL does, the flash's chip select is asserted for as
    // long as the window is in use.
    write32(ER, 0);
    write32(LQSPI_CR, LQSPI_LINEAR | LQSPI_READ);
    write32(ER, 1);
    set_cs(true);
    let mut lin = [0u8; DATA_LEN];
    for (i, w) in lin.chunks_exact_mut(4).enumerate() {
        let v = read32(LINEAR + DATA_OFFSET as usize + i * 4);
        w.copy_from_slice(&v.to_le_bytes());
    }
    set_cs(false);
    print!("LINEAR read @{:#010x}: ", LINEAR + DATA_OFFSET as usize);
    hex(&lin);
    println!();

    if fifo != lin {
        fail("FIFO and linear reads differ");
    }
    println!("FIFO and linear reads agree ({DATA_LEN} bytes)");

    // Erase the sector holding the data, program a pattern, read it back.
    // Leave linear mode first: the controller does one or the other.
    set_cs(false);
    write32(ER, 0);
    write32(LQSPI_CR, 0);
    write32(ER, 1);
    write_enable();
    let mut rx = [0u8; 4];
    transfer(&[0xD8, (DATA_OFFSET >> 16) as u8, (DATA_OFFSET >> 8) as u8, DATA_OFFSET as u8], &mut rx);
    wait_idle();
    let mut erased = [0u8; DATA_LEN];
    read_flash(DATA_OFFSET, &mut erased);
    if erased.iter().any(|&b| b != 0xFF) {
        fail("the sector is not erased after SE");
    }
    println!("ERASE sector at {DATA_OFFSET:#010x}: {DATA_LEN} bytes read back as 0xFF");

    let mut pat = [0u8; 16];
    for (i, b) in pat.iter_mut().enumerate() {
        *b = 0xA0 + i as u8;
    }
    write_enable();
    let mut tx = [0u8; 4 + 16];
    tx[0] = 0x02;
    tx[1] = (DATA_OFFSET >> 16) as u8;
    tx[2] = (DATA_OFFSET >> 8) as u8;
    tx[3] = DATA_OFFSET as u8;
    tx[4..].copy_from_slice(&pat);
    let mut rx = [0u8; 4 + 16];
    transfer(&tx, &mut rx);
    wait_idle();
    let mut back = [0u8; 16];
    read_flash(DATA_OFFSET, &mut back);
    print!("PROGRAM {} bytes at {DATA_OFFSET:#010x}, read back: ", pat.len());
    hex(&back);
    println!();
    if back != pat {
        fail("programmed bytes read back differently");
    }
    println!("RESULT: ALL PASS");
    loop {
        // SAFETY: parks the core.
        unsafe { core::arch::asm!("wfi") }
    }
}

fn print_id(id: &[u8]) {
    print!("RDID: ");
    for (i, b) in id.iter().enumerate() {
        if i > 0 {
            rt::putc(b' ');
        }
        hex(core::slice::from_ref(b));
    }
    println!();
}
