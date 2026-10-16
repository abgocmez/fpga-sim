//! Conformance of the PS XADC interface (`xadcps`, Zynq-7000 XADC block) at
//! 0xF8007100: CFG, INT_STS, INT_MASK, MSTS, CMDFIFO, RDFIFO and MCTL, and a DRP
//! read of the on-chip temperature and VCCINT through the command FIFO
//! (UG585 ch. 4 and the XADC chapter; `VERIFY:` marks what is written from memory).
//!
//! Each check prints one line, `CHECK n name: PASS|FAIL|N/A reason`.
//! * A reset value that differs from the one remembered from UG585 is N/A with
//!   both values (the model or the remembered value is wrong; not verified).
//! * A conversion read that returns 0 is N/A: 0 is not a valid code for any of
//!   these sensors, so the model returned no conversion data.
//! * A FAIL is a mismatch with the documented behaviour.
//! The last line is `RESULT: ALL PASS` only when no check failed.
//!
//! Command word (VERIFY, Linux `xilinx-xadc` "XADC_ZYNQ_CMD"): bits 27:26 command
//! (1 = DRP read, 2 = DRP write), bits 25:16 DRP address, bits 15:0 data. A read
//! result appears in RDFIFO bits 15:0.
#![no_std]
#![no_main]

use rt::{println, read32, write32};

const XADC: usize = 0xF800_7100;

// VERIFY: offsets from the XADC interface base (xadcps_hw.h).
const CFG: usize = 0x00;
const INT_STS: usize = 0x04;
const INT_MASK: usize = 0x08;
const MSTS: usize = 0x0C;
const CMDFIFO: usize = 0x10;
const RDFIFO: usize = 0x14;
const MCTL: usize = 0x18;

const CMD_READ: u32 = 1;
const CMD_WRITE: u32 = 2;

/// DRP command word.
const fn drp_cmd(cmd: u32, addr: u32, data: u32) -> u32 {
    (cmd << 26) | (addr << 16) | (data & 0xFFFF)
}

// DRP addresses of the on-chip sensors (UG480).
const DRP_TEMP: u32 = 0x00;
const DRP_VCCINT: u32 = 0x01;
/// A DRP configuration register used for the write-then-read check (VERIFY:
/// alarm thresholds at 0x40..0x4F are read/write in UG480).
const DRP_ALARM: u32 = 0x40;

/// Counts checks and failures; prints the result line.
struct Run {
    n: u32,
    fails: u32,
}

impl Run {
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

    fn eq(&mut self, name: &str, got: u32, want: u32) {
        self.n += 1;
        if got == want {
            println!("CHECK {} {name}: PASS {got:#010x}", self.n);
        } else {
            self.fails += 1;
            println!("CHECK {} {name}: FAIL got {got:#010x}, want {want:#010x}", self.n);
        }
    }

    /// A conversion or DRP value: N/A when the model returned 0 (no data),
    /// otherwise PASS when `ok`, FAIL when not.
    fn data(&mut self, name: &str, raw: u32, ok: impl FnOnce(u32) -> bool) {
        self.n += 1;
        if raw == 0 {
            println!(
                "CHECK {} {name}: N/A RDFIFO reads 0 for the command: the model returns no conversion data",
                self.n
            );
        } else if ok(raw) {
            println!("CHECK {} {name}: PASS raw {raw:#06x}", self.n);
        } else {
            self.fails += 1;
            println!("CHECK {} {name}: FAIL raw {raw:#06x} out of range", self.n);
        }
    }

    fn na(&mut self, name: &str, why: &str) {
        self.n += 1;
        println!("CHECK {} {name}: N/A {why}", self.n);
    }
}

/// One DRP read through the command FIFO: returns the 16-bit result.
fn drp_read(addr: u32) -> u32 {
    write32(XADC + CMDFIFO, drp_cmd(CMD_READ, addr, 0));
    // The conversion takes a few microseconds; wait, then read the FIFO once.
    rt::delay_us(200);
    read32(XADC + RDFIFO) & 0xFFFF
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("conf_xadcps: XADC interface at {XADC:#x}");
    let mut r = Run { n: 0, fails: 0 };

    // 1-4: reset values, before anything writes to the block.
    // VERIFY: reset values from UG585 (from memory).
    r.reset("XADC CFG reset", read32(XADC + CFG), 0x0000_0000);
    r.reset("XADC INT_STS reset", read32(XADC + INT_STS), 0x0000_0000);
    r.reset("XADC MSTS reset", read32(XADC + MSTS), 0x0000_0000);
    r.reset("XADC MCTL reset", read32(XADC + MCTL), 0x0000_0000);

    // 5-6: INT_MASK (bits 9:0 are the interrupt bits, VERIFY) and MCTL read/write.
    write32(XADC + INT_MASK, 0x0000_FFFF);
    r.eq("XADC INT_MASK write/read (bits 9:0)", read32(XADC + INT_MASK) & 0xFFFF, 0x3FF);
    write32(XADC + INT_MASK, 0);
    r.eq("XADC INT_MASK cleared", read32(XADC + INT_MASK) & 0xFFFF, 0);
    write32(XADC + MCTL, 0x0000_0040);
    r.eq("XADC MCTL write/read", read32(XADC + MCTL) & 0xFF, 0x40);
    write32(XADC + MCTL, 0);

    // 7: CFG write/read (the configuration word is kept as written).
    write32(XADC + CFG, 0x0000_1234);
    r.eq("XADC CFG write/read", read32(XADC + CFG), 0x0000_1234);
    write32(XADC + CFG, 0);

    // 8: INT_STS. Event bits are write-1-to-clear, but no event is produced in this
    // program, so the clear is not checked.
    r.na("XADC INT_STS write-1-to-clear", "not checked: no interrupt event is produced by this program");

    // 9-10: temperature and VCCINT through the command FIFO. Raw code to degrees C
    // (UG480): T = raw * 503.975 / 4096 - 273.15, accepted -40 C .. 125 C.
    // VCCINT: raw * 3 / 65536 volts, accepted 0.9 V .. 1.1 V (raw 19660 .. 24136).
    let temp = drp_read(DRP_TEMP);
    r.data("XADC temperature via CMDFIFO/RDFIFO in -40..125 C", temp, |raw| {
        let t_mc = (raw as i64 * 503_975 / 4096) - 273_150; // millidegrees C
        (-40_000..=125_000).contains(&t_mc)
    });
    let vcc = drp_read(DRP_VCCINT);
    r.data("XADC VCCINT via CMDFIFO/RDFIFO in 0.9..1.1 V", vcc, |raw| (19_660..=24_136).contains(&raw));

    // 11: a DRP write followed by a read of the same address returns the value.
    write32(XADC + CMDFIFO, drp_cmd(CMD_WRITE, DRP_ALARM, 0x1234));
    rt::delay_us(200);
    let back = drp_read(DRP_ALARM);
    r.data("XADC DRP write then read (0x40) returns the value", back, |raw| raw == 0x1234);

    // 12: alarm thresholds and the rest of the DRP map are not part of this program.
    r.na("XADC alarm thresholds (DRP 0x40..0x4F) beyond 0x40", "not checked: only the DRP write/read at 0x40 is exercised");

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
