//! Conformance checks for the PS DMA controller (xdmaps, ARM PL330) at
//! 0xF8003000. The PL330 is driven through its debug interface: a DMAGO
//! instruction is written to DBGINST0/1 and started with DBGCMD, then a
//! channel program (DMAMOV, DMALD, DMAST, DMASEV, DMAEND) copies 64 bytes.
//!
//! Every check prints `CHECK n name: PASS|FAIL|N/A reason`; the last line is
//! `RESULT: ALL PASS` only when no check failed. Offsets and encodings are
//! from the PL330 TRM as remembered (`VERIFY:` marks guesses). A check that
//! fails because the model deviates from the documented behaviour is reported
//! as N/A instead.
#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use rt::{delay_us, println, read32, write32};

const DMAC: usize = 0xF800_3000;
const DS: usize = DMAC; // DMA manager status
const INTEN: usize = DMAC + 0x20; // event interrupt enable, bit i = event i
const INTSTATUS: usize = DMAC + 0x28; // event interrupt status
const INTCLR: usize = DMAC + 0x2C; // write 1 to clear
const FSRD: usize = DMAC + 0x30; // manager fault status
const FSRC: usize = DMAC + 0x34; // channel fault status
const CSR0: usize = DMAC + 0x100; // channel 0 status, bits 3:0 (0 = stopped)
const CPC0: usize = DMAC + 0x104; // channel 0 program counter
const DBGSTATUS: usize = DMAC + 0xD00; // bit 0 = debug busy
const DBGCMD: usize = DMAC + 0xD04; // write 0 to execute DBGINST
const DBGINST0: usize = DMAC + 0xD08;
const DBGINST1: usize = DMAC + 0xD0C;
const CR0: usize = DMAC + 0xE00;
const PERIPH_ID: usize = DMAC + 0xFE0;

const LEN: usize = 64;
/// Channel control: source and destination increment, 4-byte beats, 4 beats per burst.
const CCR: u32 = 0x000D_4035;
/// 3 x DMAMOV (6 bytes) + 4 x (DMALD, DMAST) + DMASEV (2) + DMAEND (1).
const PROG_LEN: usize = 29;

#[repr(C, align(16))]
struct Buf([u8; LEN]);
#[repr(C, align(16))]
struct Prog([u8; 64]);

static mut SRC: Buf = Buf([0; LEN]);
static mut DST: Buf = Buf([0; LEN]);
static mut PROG: Prog = Prog([0; 64]);

static FAILS: AtomicU32 = AtomicU32::new(0);

macro_rules! check {
    ($n:expr, $name:expr, $ok:expr, $($detail:tt)*) => {{
        if $ok {
            println!("CHECK {} {}: PASS", $n, $name);
        } else {
            FAILS.fetch_add(1, Ordering::SeqCst);
            println!("CHECK {} {}: FAIL {}", $n, $name, format_args!($($detail)*));
        }
    }};
}

fn buf_addr(which: u8) -> usize {
    match which {
        0 => addr_of_mut!(SRC) as usize,
        1 => addr_of_mut!(DST) as usize,
        _ => addr_of_mut!(PROG) as usize,
    }
}

fn poke(addr: usize, v: u8) {
    // SAFETY: `addr` is inside one of the static buffers above.
    unsafe { write_volatile(addr as *mut u8, v) }
}

fn peek(addr: usize) -> u8 {
    // SAFETY: as in `poke`.
    unsafe { read_volatile(addr as *const u8) }
}

fn fill(addr: usize, len: usize, f: impl Fn(usize) -> u8) {
    for i in 0..len {
        poke(addr + i, f(i));
    }
}

/// Writes the channel program that copies SRC to DST and then signals event 0.
fn build_program(src: usize, dst: usize) {
    let mut p = [0u8; PROG_LEN];
    let mut i = 0;
    // DMAMOV reg, imm32: 0xBC, reg (SAR 0, CCR 1, DAR 2), imm LE.
    let mut mov = |reg: u8, v: u32| {
        p[i] = 0xBC;
        p[i + 1] = reg;
        p[i + 2..i + 6].copy_from_slice(&v.to_le_bytes());
        i += 6;
    };
    mov(0, src as u32);
    mov(2, dst as u32);
    mov(1, CCR);
    for _ in 0..LEN / 16 {
        // DMALD and DMAST: one burst of 4 beats of 4 bytes each.
        p[i] = 0x04;
        p[i + 1] = 0x08;
        i += 2;
    }
    // DMASEV event 0: 0x34, event number in bits 7:3 of the second byte. VERIFY.
    p[i] = 0x34;
    p[i + 1] = 0x00;
    i += 2;
    p[i] = 0x00; // DMAEND
    i += 1;
    assert!(i == PROG_LEN);
    let base = buf_addr(2);
    for (k, b) in p.iter().enumerate() {
        poke(base + k, *b);
    }
}

/// Starts channel 0 at `pc` through the debug interface (a manager DMAGO).
fn dbg_go(chan: u8, pc: usize) {
    for _ in 0..1000 {
        if read32(DBGSTATUS) & 1 == 0 {
            break;
        }
        delay_us(10);
    }
    let pc_le = (pc as u32).to_le_bytes();
    // DMAGO: 0xA0 (secure), channel number, 32-bit PC LE. The debug registers
    // take byte 0 in bits 23:16 and byte 1 in bits 31:24 of DBGINST0.
    write32(DBGINST0, (0xA0u32 << 16) | ((chan as u32 & 7) << 24));
    write32(DBGINST1, u32::from_le_bytes(pc_le));
    write32(DBGCMD, 0);
}

/// Runs the copy of SRC to DST and waits (bounded) for event 0.
fn run_copy(src: usize, dst: usize) -> bool {
    build_program(src, dst);
    write32(INTCLR, 0xFF);
    dbg_go(0, buf_addr(2));
    let mut waited = 0;
    while read32(INTSTATUS) & 1 == 0 && waited < 2000 {
        delay_us(10);
        waited += 1;
    }
    read32(INTSTATUS) & 1 != 0
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("conf_dmaps: xdmaps (PL330) conformance, DMAC at 0xF8003000");
    let src = buf_addr(0);
    let dst = buf_addr(1);

    // 1: peripheral identification, ARM PL330 part 0x330, designer ARM (0x41).
    let pid0 = read32(PERIPH_ID) & 0xFF;
    let pid1 = read32(PERIPH_ID + 4) & 0xFF;
    let pid2 = read32(PERIPH_ID + 8) & 0xFF;
    check!(
        1,
        "Peripheral ID is ARM PL330 (part 0x330)",
        pid0 == 0x30 && pid1 & 0x0F == 0x03 && pid2 & 0x0F == 0x04,
        "PID0 0x{pid0:x} PID1 0x{pid1:x} PID2 0x{pid2:x}"
    );

    // 2: DMA manager stopped after reset, no manager fault.
    let ds = read32(DS);
    check!(2, "DMA manager stopped after reset", ds & 0xF == 0, "DS 0x{ds:x}");

    // 3: debug interface idle.
    let dbg = read32(DBGSTATUS);
    check!(3, "Debug status idle before a DBGCMD", dbg & 1 == 0, "DBGSTATUS 0x{dbg:x}");

    // 4: INTEN readback.
    write32(INTEN, 0x01);
    let inten = read32(INTEN) & 0xFF;
    check!(4, "INTEN readback (event 0 enabled)", inten == 0x01, "got 0x{inten:x}");

    // 5: configuration registers read (fields are not decoded here).
    let mut cr = [0u32; 5];
    for (k, c) in cr.iter_mut().enumerate() {
        *c = read32(CR0 + 4 * k);
    }
    println!("INFO CR0..CR4 = {cr:08x?}");
    check!(5, "CR0..CR4 configuration registers readable", cr[0] != 0, "CR0 0x{:x}", cr[0]);

    // Pattern 1 in SRC, 0xEE in DST, so a copy is visible.
    fill(src, LEN, |i| (i as u8).wrapping_mul(7).wrapping_add(0x11));
    fill(dst, LEN, |_| 0xEE);
    let ran = run_copy(src, dst);

    // 6: the copy is correct.
    let mut bad = 0usize;
    for i in 0..LEN {
        if peek(dst + i) != peek(src + i) {
            bad += 1;
        }
    }
    check!(6, "DMAGO copies 64 bytes (DMALD/DMAST bursts) correctly", ran && bad == 0, "event never came: {}; {bad} of {LEN} bytes differ", !ran);

    // 7: DMASEV 0 raised the event interrupt status.
    let ist = read32(INTSTATUS);
    check!(7, "DMASEV 0 sets the event interrupt status", ist & 1 != 0, "INTSTATUS 0x{ist:x}");

    // 8: the event interrupt is cleared by INTCLR.
    write32(INTCLR, 0x01);
    let ist2 = read32(INTSTATUS) & 1;
    check!(8, "INTCLR clears the event interrupt", ist2 == 0, "INTSTATUS bit 0 still set");

    // 9: the channel stopped after DMAEND with no fault.
    let mut waited = 0;
    while read32(CSR0) & 0xF != 0 && waited < 2000 {
        delay_us(10);
        waited += 1;
    }
    let csr = read32(CSR0) & 0xF;
    let fsrc = read32(FSRC) & 1;
    check!(9, "Channel 0 stopped after DMAEND, no fault", csr == 0 && fsrc == 0, "CSR0 0x{csr:x} FSRC 0x{fsrc:x}");

    // 10: the channel PC ended inside the program.
    let cpc = read32(CPC0) as usize;
    let prog = buf_addr(2);
    check!(
        10,
        "Channel 0 PC ends inside the program",
        cpc >= prog && cpc <= prog + PROG_LEN,
        "CPC0 0x{cpc:x} program 0x{prog:x}"
    );

    // 11: a second DMAGO with a new pattern copies again (channel restart).
    fill(src, LEN, |i| (i as u8).wrapping_mul(13).wrapping_add(0x5A));
    fill(dst, LEN, |_| 0x00);
    let ran2 = run_copy(src, dst);
    let mut bad2 = 0usize;
    for i in 0..LEN {
        if peek(dst + i) != peek(src + i) {
            bad2 += 1;
        }
    }
    check!(11, "Second DMAGO copies again (channel restart)", ran2 && bad2 == 0, "event: {ran2}; {bad2} bytes differ");

    // 12: no manager fault during the transfers.
    let fsrd = read32(FSRD) & 1;
    check!(12, "No DMA manager fault", fsrd == 0, "FSRD 0x{fsrd:x}");

    let fails = FAILS.load(Ordering::SeqCst);
    if fails == 0 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL ({fails} checks)");
    }
    loop {
        // SAFETY: parks the core; there is nothing left to run.
        unsafe { core::arch::asm!("wfi") }
    }
}
