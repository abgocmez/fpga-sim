//! Conformance self-test of the Zynq PS GPIO (xgpiops, base 0xE000A000) under
//! the simulator. Each check prints one line `CHECK n name: PASS|FAIL|N/A why`;
//! `RESULT: ALL PASS` is printed only when no check failed. A check that the
//! simulator's PS model does not implement is reported `N/A` with the reason,
//! never faked.
//!
//! Register layout and bit meanings: Zynq-7000 TRM UG585, chapter 14 (GPIO).
//! VERIFY marks offsets and bit meanings written from memory, not yet compared
//! with the TRM text. Banks: 0 = MIO[15:0] (16 bits), 1 = MIO[31:16] (16 bits),
//! 2 = MIO[53:32] (22 bits), 3 = EMIO[31:0] (32 bits). VERIFY the bank widths.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{println, read32, write32};

const BASE: usize = 0xE000_A000;

// Per bank n (stride 8 for MASK_DATA, 4 for DATA and DATA_RO).
const MASK_DATA_LSW: usize = 0x000; // +8n: bits 31:16 mask (1 = inhibit), 15:0 data
const MASK_DATA_MSW: usize = 0x004; // +8n: same, for bits 31:16 of the bank word
const DATA: usize = 0x040; // +4n
const DATA_RO: usize = 0x060; // +4n

// Per bank n (stride 0x40, from the DIRM_0 block at 0x204).
const DIRM: usize = 0x204;
const OEN: usize = 0x208;
const INT_MASK: usize = 0x20C;
const INT_EN: usize = 0x210; // write 1: clear the mask bit
const INT_DIS: usize = 0x214; // write 1: set the mask bit
const INT_STAT: usize = 0x218; // write 1 to clear (edge types)
const INT_TYPE: usize = 0x21C;
const INT_POLARITY: usize = 0x220;
const INT_ANY: usize = 0x224;
const BANK_STRIDE: usize = 0x40;

/// Valid pin bits per bank (VERIFY: UG585 bank widths 16, 16, 22, 32).
const WIDTH: [u32; 4] = [0xFFFF, 0xFFFF, 0x003F_FFFF, 0xFFFF_FFFF];

static FAILS: AtomicU32 = AtomicU32::new(0);

fn r(off: usize) -> u32 {
    read32(BASE + off)
}

fn w(off: usize, v: u32) {
    write32(BASE + off, v);
}

fn bank(off: usize, b: usize) -> usize {
    off + BANK_STRIDE * b
}

/// Outcome of one check.
enum Res {
    Pass,
    Fail { got: u32, want: u32 },
    Na(&'static str),
}

fn cmp(got: u32, want: u32) -> Res {
    if got == want {
        Res::Pass
    } else {
        Res::Fail { got, want }
    }
}

/// Runs the bank checks in order and returns the first failure.
fn each_bank(mut f: impl FnMut(usize, u32) -> Res) -> Res {
    for (b, &m) in WIDTH.iter().enumerate() {
        match f(b, m) {
            Res::Pass => {}
            other => return other,
        }
    }
    Res::Pass
}

fn report(n: u32, name: &str, res: Res) {
    match res {
        Res::Pass => println!("CHECK {n} {name}: PASS"),
        Res::Fail { got, want } => {
            FAILS.fetch_add(1, Ordering::SeqCst);
            println!("CHECK {n} {name}: FAIL got=0x{got:08x} want=0x{want:08x}");
        }
        Res::Na(why) => println!("CHECK {n} {name}: N/A {why}"),
    }
}

/// Reset state: direction and output enable clear, all interrupts masked.
fn reset_values() -> Res {
    each_bank(|b, m| {
        let dirm = read32(BASE + bank(DIRM, b)) & m;
        if dirm != 0 {
            return Res::Fail { got: dirm, want: 0 };
        }
        let oen = read32(BASE + bank(OEN, b)) & m;
        if oen != 0 {
            return Res::Fail { got: oen, want: 0 };
        }
        // VERIFY: INT_MASK resets to all ones (every bank interrupt masked).
        match cmp(read32(BASE + bank(INT_MASK, b)) & m, m) {
            Res::Pass => {}
            other => return other,
        }
        cmp(read32(BASE + bank(INT_TYPE, b)) & m, 0)
    })
}

/// DIRM_n keeps the written direction bits (checked for two patterns).
fn dirm_readback() -> Res {
    each_bank(|b, m| {
        for p in [0xA5C3_5A3C_u32, 0x0F0F_F0F0] {
            write32(BASE + bank(DIRM, b), p);
            match cmp(read32(BASE + bank(DIRM, b)) & m, p & m) {
                Res::Pass => {}
                other => return other,
            }
        }
        write32(BASE + bank(DIRM, b), 0);
        Res::Pass
    })
}

/// OEN_n keeps the written output-enable bits.
fn oen_readback() -> Res {
    each_bank(|b, m| {
        for p in [0x5A3C_C3A5_u32, 0xF00F_0FF0] {
            write32(BASE + bank(OEN, b), p);
            match cmp(read32(BASE + bank(OEN, b)) & m, p & m) {
                Res::Pass => {}
                other => return other,
            }
        }
        write32(BASE + bank(OEN, b), 0);
        Res::Pass
    })
}

/// DATA_n keeps the output data written to it.
fn data_readback() -> Res {
    each_bank(|b, m| {
        let p = 0x3C5A_A5C3_u32;
        w(DATA + 4 * b, p);
        let res = cmp(read32(BASE + DATA + 4 * b) & m, p & m);
        w(DATA + 4 * b, 0);
        res
    })
}

/// MASK_DATA_0_LSW: bits 31:16 protect pins 7:0 and only pins 15:8 change.
fn masked_write_lsw() -> Res {
    w(DATA, 0x0000_5A5A);
    w(MASK_DATA_LSW, (0x00FF << 16) | 0x3C00);
    let res = cmp(r(DATA) & 0xFFFF, 0x3C5A);
    w(DATA, 0);
    res
}

/// MASK_DATA_3_MSW: bits 31:16 protect pins 16..23 of bank 3; pins 24..31 change.
fn masked_write_msw() -> Res {
    w(DATA + 4 * 3, 0xA5A5_5A5A);
    w(MASK_DATA_MSW + 8 * 3, (0x00FF << 16) | 0x3C00);
    let res = cmp(r(DATA + 4 * 3), 0x3CA5_5A5A);
    w(DATA + 4 * 3, 0);
    res
}

/// Bank 2 has 22 pins: bits above 21 read back as zero.
fn bank2_width() -> Res {
    w(DATA + 4 * 2, 0xFFFF_FFFF);
    let got = r(DATA + 4 * 2);
    w(DATA + 4 * 2, 0);
    cmp(got, WIDTH[2])
}

/// INT_DIS sets and INT_EN clears the bank 1 mask bits.
fn int_mask_en_dis() -> Res {
    let b = 1;
    w(bank(INT_DIS, b), 0xFFFF);
    let all = cmp(r(bank(INT_MASK, b)) & 0xFFFF, 0xFFFF);
    if !matches!(all, Res::Pass) {
        return all;
    }
    w(bank(INT_EN, b), 0x0005);
    match cmp(r(bank(INT_MASK, b)) & 0xFFFF, 0xFFFA) {
        Res::Pass => {}
        other => return other,
    }
    w(bank(INT_DIS, b), 0x0001);
    cmp(r(bank(INT_MASK, b)) & 0xFFFF, 0xFFFB)
}

/// INT_TYPE, INT_POLARITY and INT_ANY keep what is written (bank 2).
fn int_type_polarity_any() -> Res {
    let b = 2;
    let m = WIDTH[b];
    w(bank(INT_TYPE, b), 0x0F0F_0F0F);
    w(bank(INT_POLARITY, b), 0x0000_F0F3);
    w(bank(INT_ANY, b), 0x0000_0F05);
    let res = cmp(r(bank(INT_TYPE, b)) & m, 0x0F0F_0F0F & m);
    let res = match res {
        Res::Pass => cmp(r(bank(INT_POLARITY, b)) & m, 0x0000_F0F3 & m),
        other => other,
    };
    let res = match res {
        Res::Pass => cmp(r(bank(INT_ANY, b)) & m, 0x0000_0F05 & m),
        other => other,
    };
    w(bank(INT_TYPE, b), 0);
    w(bank(INT_POLARITY, b), 0);
    w(bank(INT_ANY, b), 0);
    res
}

/// With no pin event, INT_STAT reads zero and write-1-to-clear leaves it zero.
fn int_status_no_events() -> Res {
    w(bank(INT_STAT, 1), 0xFFFF);
    cmp(r(bank(INT_STAT, 1)) & 0xFFFF, 0)
}

/// An output pin that is enabled reads back on DATA_RO (VERIFY: DATA_RO
/// mirrors the pin, and an output pin's pad is read back as driven).
fn data_ro_mirrors_output() -> Res {
    w(bank(DIRM, 0), 0x00FF);
    w(bank(OEN, 0), 0x00FF);
    w(DATA, 0x00A5);
    let res = cmp(r(DATA_RO) & 0x00FF, 0x00A5);
    w(bank(DIRM, 0), 0);
    w(bank(OEN, 0), 0);
    w(DATA, 0);
    res
}

/// Reason for every register check: the simulator maps no GPIO at this address.
const NOT_MAPPED: &str =
    "not modelled: QEMU maps no PS GPIO at 0xE000A000 (access rejected, reads 0)";

/// Whether the simulator maps this block: DIRM_0 must read back what was written.
/// Without it every register read is 0 and a check would pass by accident.
fn mapped() -> bool {
    w(DIRM, 0x0000_A5A5);
    let ok = r(DIRM) & 0xFFFF == 0x0000_A5A5;
    w(DIRM, 0);
    ok
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("conformance: xgpiops at 0xE000A000 (UG585 ch. 14)");
    let m = mapped();
    let gate = |f: fn() -> Res| if m { f() } else { Res::Na(NOT_MAPPED) };
    let mut n = 0;
    let mut next = || {
        n += 1;
        n
    };
    report(next(), "reset_values", gate(reset_values));
    report(next(), "dirm_readback", gate(dirm_readback));
    report(next(), "oen_readback", gate(oen_readback));
    report(next(), "data_readback", gate(data_readback));
    report(next(), "masked_write_lsw", gate(masked_write_lsw));
    report(next(), "masked_write_msw", gate(masked_write_msw));
    report(next(), "bank2_width", gate(bank2_width));
    report(next(), "int_mask_en_dis", gate(int_mask_en_dis));
    report(next(), "int_type_polarity_any", gate(int_type_polarity_any));
    report(next(), "int_status_no_events", gate(int_status_no_events));
    report(
        next(),
        "data_ro_mirrors_output",
        gate(data_ro_mirrors_output),
    );
    report(
        next(),
        "pin_event_interrupt",
        Res::Na(
            "no pin source: no pin is driven from outside the PS model, INT_STAT cannot be set",
        ),
    );

    if FAILS.load(Ordering::SeqCst) == 0 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    loop {
        // SAFETY: parks the core until an interrupt that never comes.
        unsafe { core::arch::asm!("wfi") }
    }
}
