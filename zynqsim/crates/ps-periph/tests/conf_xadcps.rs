//! The checks of `firmware/conf_xadcps`, encoded as host tests against the XADC interface at
//! 0xF8007100 (inside the devcfg window of `ps-periph::devcfg`). Names and expected values are the
//! firmware's, with two documented differences.
//!
//! * Check 10 (temperature). The firmware applies `T = raw * 503.975 / 4096 - 273.15` to the
//!   RDFIFO value. The UG480 data format puts the 12-bit code in bits 15:4 of the 16-bit result,
//!   and the firmware's own VCCINT check (11) reads it that way (19660..24136 is the left-aligned
//!   range). Applied literally to the model's UG480 value the temperature formula gives about
//!   4.5 million millidegrees, so the literal check FAILs; the test asserts that, and then runs the
//!   check on the code (`raw >> 4`), which is what the firmware means.
//! * Check 13 (alarm registers 0x41..0x4F) is N/A in the firmware; here it is run.
//! * Check 9 (INT_STS write-1-to-clear) stays N/A: the model produces no XADC event, so there is
//!   no bit to clear. A write of 1 to the clear register is checked not to set anything.

mod support;

use mem_sys::SystemBus;
use support::*;

const XADC: u64 = 0xF800_7100;
const CFG: u64 = 0x00;
const INT_STS: u64 = 0x04;
const INT_MASK: u64 = 0x08;
const MSTS: u64 = 0x0C;
const CMDFIFO: u64 = 0x10;
const RDFIFO: u64 = 0x14;
const MCTL: u64 = 0x18;

const CMD_READ: u32 = 1;
const CMD_WRITE: u32 = 2;
const DRP_TEMP: u32 = 0x00;
const DRP_VCCINT: u32 = 0x01;

fn drp_cmd(cmd: u32, addr: u32, data: u32) -> u32 {
    (cmd << 26) | (addr << 16) | (data & 0xFFFF)
}

fn drp_read(bus: &mut SystemBus, addr: u32) -> u32 {
    wr(bus, XADC + CMDFIFO, drp_cmd(CMD_READ, addr, 0));
    tick_us(bus, 200);
    rd(bus, XADC + RDFIFO) & 0xFFFF
}

type Res = Result<(), String>;

fn check(ok: bool, detail: impl FnOnce() -> String) -> Res {
    if ok { Ok(()) } else { Err(detail()) }
}

#[test]
fn conf_xadcps_checks_pass_on_the_model() {
    let (mut bus, _host) = system();
    let mut checks: Vec<(&str, Res)> = Vec::new();

    // Host values the sensors report: 25 C and 1.0 V (the model's defaults).
    checks.push((
        "1 XADC CFG reset",
        check(rd(&mut bus, XADC + CFG) == 0, || "nonzero".into()),
    ));
    checks.push((
        "2 XADC INT_STS reset",
        check(rd(&mut bus, XADC + INT_STS) == 0, || "nonzero".into()),
    ));
    checks.push((
        "3 XADC MSTS reset",
        check(rd(&mut bus, XADC + MSTS) == 0, || "nonzero".into()),
    ));
    checks.push((
        "4 XADC MCTL reset",
        check(rd(&mut bus, XADC + MCTL) == 0, || "nonzero".into()),
    ));

    wr(&mut bus, XADC + INT_MASK, 0xFFFF);
    let got = rd(&mut bus, XADC + INT_MASK) & 0xFFFF;
    checks.push((
        "5 XADC INT_MASK write/read (bits 9:0)",
        check(got == 0x3FF, || format!("{got:#x}")),
    ));
    wr(&mut bus, XADC + INT_MASK, 0);
    let got = rd(&mut bus, XADC + INT_MASK) & 0xFFFF;
    checks.push(("6 XADC INT_MASK cleared", check(got == 0, || format!("{got:#x}"))));
    wr(&mut bus, XADC + MCTL, 0x40);
    let got = rd(&mut bus, XADC + MCTL) & 0xFF;
    checks.push(("7 XADC MCTL write/read", check(got == 0x40, || format!("{got:#x}"))));
    wr(&mut bus, XADC + MCTL, 0);

    wr(&mut bus, XADC + CFG, 0x1234);
    let got = rd(&mut bus, XADC + CFG);
    checks.push(("8 XADC CFG write/read", check(got == 0x1234, || format!("{got:#x}"))));
    wr(&mut bus, XADC + CFG, 0);

    // Check 9: N/A in the firmware (no event source). Here a clear must not set anything.
    wr(&mut bus, XADC + INT_STS, 0x3FF);
    let sts = rd(&mut bus, XADC + INT_STS);
    checks.push((
        "9 XADC INT_STS write-1-to-clear (no event source: nothing is set)",
        check(sts == 0, || format!("INT_STS set by a clear: {sts:#x}")),
    ));

    // Check 10: the firmware's literal formula on the UG480 value FAILs; the code formula passes.
    let temp = drp_read(&mut bus, DRP_TEMP);
    let literal_mc = (i64::from(temp) * 503_975 / 4096) - 273_150;
    checks.push((
        "10a documented: the firmware's literal formula (raw bits 15:0) is out of range",
        check(!(-40_000..=125_000).contains(&literal_mc), || {
            format!("literal formula is in range for raw {temp:#x}: {literal_mc} mC")
        }),
    ));
    let code = temp >> 4;
    let t_mc = (i64::from(code) * 503_975 / 4096) - 273_150;
    checks.push((
        "10 XADC temperature via CMDFIFO/RDFIFO in -40..125 C (code = raw >> 4)",
        check((-40_000..=125_000).contains(&t_mc), || format!("raw {temp:#x} -> {t_mc} mC")),
    ));
    checks.push((
        "10b XADC temperature near the host value 25 C",
        check((t_mc - 25_000).abs() < 500, || format!("{t_mc} mC")),
    ));

    let vcc = drp_read(&mut bus, DRP_VCCINT);
    checks.push((
        "11 XADC VCCINT via CMDFIFO/RDFIFO in 0.9..1.1 V",
        check((19_660..=24_136).contains(&vcc), || format!("raw {vcc:#x}")),
    ));

    wr(&mut bus, XADC + CMDFIFO, drp_cmd(CMD_WRITE, 0x40, 0x1234));
    tick_us(&mut bus, 200);
    let back = drp_read(&mut bus, 0x40);
    checks.push((
        "12 XADC DRP write then read (0x40) returns the value",
        check(back == 0x1234, || format!("{back:#x}")),
    ));

    // Check 13 (N/A in the firmware): the alarm thresholds 0x41..0x4F are read/write.
    let mut bad = None;
    for a in 0x41..=0x4Fu32 {
        wr(&mut bus, XADC + CMDFIFO, drp_cmd(CMD_WRITE, a, 0x0A00 + a));
        let got = drp_read(&mut bus, a);
        if got != 0x0A00 + a && bad.is_none() {
            bad = Some((a, got));
        }
    }
    checks.push((
        "13 XADC alarm thresholds (DRP 0x41..0x4F) read back",
        check(bad.is_none(), || format!("{bad:?}")),
    ));
    let mut failed = Vec::new();
    for (name, res) in &checks {
        match res {
            Ok(()) => println!("CHECK {name}: PASS"),
            Err(why) => {
                println!("CHECK {name}: FAIL {why}");
                failed.push(*name);
            }
        }
    }
    assert_eq!(failed, Vec::<&str>::new(), "failed checks");
    assert_eq!(checks.len(), 15);
}
