//! The checks of `firmware/conf_iicps`, encoded as host tests against the I2C models at
//! 0xE0004000 (I2C0, GIC 57) and 0xE0005000 (I2C1, GIC 80). Names and expected values are the
//! firmware's. The firmware's check 18 (read from an attached device) is N/A there because no
//! device is attached; here a small EEPROM-like peer is attached at 0x51 and the read is PASS.
//! The model completes a transfer inside the register write that starts it, so the firmware's
//! polling loops are not needed.

mod support;

use mem_sys::SystemBus;
use ps_periph::i2c::{I2cPeer, Nack};
use support::*;

const I2C0: u64 = 0xE000_4000;
const I2C1: u64 = 0xE000_5000;
const CONTROL: u64 = 0x00;
const STATUS: u64 = 0x04;
const ADDR: u64 = 0x08;
const DATA: u64 = 0x0C;
const ISR: u64 = 0x10;
const TRANS_SIZE: u64 = 0x14;
const SLV_PAUSE: u64 = 0x18;
const TIMEOUT: u64 = 0x1C;
const IMR: u64 = 0x20;
const IER: u64 = 0x24;
const IDR: u64 = 0x28;

const CTRL_RW: u32 = 1 << 0;
const CTRL_MS: u32 = 1 << 1;
const CTRL_NEA: u32 = 1 << 2;
const CTRL_ACKEN: u32 = 1 << 3;
const CTRL_HOLD: u32 = 1 << 4;
const CTRL_CLR_FIFO: u32 = 1 << 6;
const IXR_COMP: u32 = 1 << 0;
const IXR_NACK: u32 = 1 << 2;
const IXR_ALL: u32 = 0x2FF;
const ADDR_ABSENT: u32 = 0x50;

/// A byte device at one address: keeps what it is written and answers reads with `fill`.
struct Eeprom {
    addr: u8,
    fill: u8,
}

impl I2cPeer for Eeprom {
    fn write(&mut self, addr: u8, _data: &[u8]) -> Result<(), Nack> {
        if addr == self.addr { Ok(()) } else { Err(Nack) }
    }
    fn read(&mut self, addr: u8, len: usize) -> Result<Vec<u8>, Nack> {
        if addr == self.addr {
            Ok(vec![self.fill; len])
        } else {
            Err(Nack)
        }
    }
}

type Res = Result<(), String>;

fn check(ok: bool, detail: impl FnOnce() -> String) -> Res {
    if ok { Ok(()) } else { Err(detail()) }
}

/// Master write of one byte to `addr`; returns ISR after the transfer.
fn write_byte(bus: &mut SystemBus, base: u64, addr: u32, byte: u8) -> u32 {
    wr(bus, base + ISR, 0x3FF);
    wr(
        bus,
        base + CONTROL,
        CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_CLR_FIFO,
    );
    wr(bus, base + ADDR, addr);
    wr(bus, base + DATA, u32::from(byte));
    wr(bus, base + TRANS_SIZE, 1);
    rd(bus, base + ISR)
}

/// Master read of one byte from `addr`; returns ISR after the transfer.
fn read_byte(bus: &mut SystemBus, base: u64, addr: u32) -> u32 {
    wr(bus, base + ISR, 0x3FF);
    wr(
        bus,
        base + CONTROL,
        CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_CLR_FIFO | CTRL_RW,
    );
    wr(bus, base + ADDR, addr);
    wr(bus, base + TRANS_SIZE, 1);
    rd(bus, base + ISR)
}

fn reset_checks(bus: &mut SystemBus) -> Vec<(&'static str, Res)> {
    vec![
        (
            "1 I2C0 CONTROL reset",
            check(rd(bus, I2C0 + CONTROL) == 0, || "nonzero".into()),
        ),
        (
            "2 I2C0 STATUS reset",
            check(rd(bus, I2C0 + STATUS) == 0, || "nonzero".into()),
        ),
        (
            "3 I2C0 ISR reset",
            check(rd(bus, I2C0 + ISR) & IXR_ALL == 0, || "nonzero".into()),
        ),
        (
            "4 I2C0 IMR reset (all masked)",
            check(rd(bus, I2C0 + IMR) & IXR_ALL == IXR_ALL, || {
                format!("{:#x}", rd(bus, I2C0 + IMR))
            }),
        ),
        (
            "5 I2C0 TIMEOUT reset",
            check(rd(bus, I2C0 + TIMEOUT) & 0xFF == 0x1F, || "not 0x1F".into()),
        ),
        (
            "6 I2C0 TRANS_SIZE reset",
            check(rd(bus, I2C0 + TRANS_SIZE) & 0xFF == 0, || "nonzero".into()),
        ),
        (
            "7 I2C0 ADDR reset",
            check(rd(bus, I2C0 + ADDR) & 0x3FF == 0, || "nonzero".into()),
        ),
    ]
}

fn config_checks(bus: &mut SystemBus) -> Vec<(&'static str, Res)> {
    let mut out = Vec::new();
    wr(
        bus,
        I2C0 + CONTROL,
        CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_HOLD,
    );
    let got = rd(bus, I2C0 + CONTROL) & 0x7F;
    let want = CTRL_MS | CTRL_NEA | CTRL_ACKEN | CTRL_HOLD;
    out.push((
        "8 I2C0 CONTROL write/read (MS NEA ACKEN HOLD)",
        check(got == want, || format!("{got:#x}")),
    ));
    wr(bus, I2C0 + CONTROL, CTRL_MS | CTRL_HOLD);
    let hold_on = rd(bus, I2C0 + CONTROL) & CTRL_HOLD;
    wr(bus, I2C0 + CONTROL, CTRL_MS);
    let hold_off = rd(bus, I2C0 + CONTROL) & CTRL_HOLD;
    out.push((
        "9 I2C0 HOLD bit set and cleared",
        check(hold_on | hold_off == CTRL_HOLD, || {
            format!("{hold_on:#x} {hold_off:#x}")
        }),
    ));
    wr(bus, I2C0 + ADDR, ADDR_ABSENT);
    out.push((
        "10 I2C0 ADDR write/read",
        check(rd(bus, I2C0 + ADDR) & 0x3FF == ADDR_ABSENT, || {
            "mismatch".into()
        }),
    ));
    wr(bus, I2C0 + TRANS_SIZE, 0x03);
    out.push((
        "11 I2C0 TRANS_SIZE write/read",
        check(rd(bus, I2C0 + TRANS_SIZE) & 0xFF == 0x03, || {
            "mismatch".into()
        }),
    ));
    wr(bus, I2C0 + SLV_PAUSE, 0x05);
    out.push((
        "12 I2C0 Slave_mon_pause write/read",
        check(rd(bus, I2C0 + SLV_PAUSE) & 0xFF == 0x05, || {
            "mismatch".into()
        }),
    ));
    wr(bus, I2C0 + TIMEOUT, 0x0A);
    out.push((
        "13 I2C0 Time_out write/read",
        check(rd(bus, I2C0 + TIMEOUT) & 0xFF == 0x0A, || "mismatch".into()),
    ));
    wr(bus, I2C0 + TIMEOUT, 0x1F);

    wr(bus, I2C0 + IER, IXR_COMP);
    let enabled = rd(bus, I2C0 + IMR) & IXR_COMP;
    wr(bus, I2C0 + IDR, IXR_COMP);
    let disabled = rd(bus, I2C0 + IMR) & IXR_COMP;
    out.push((
        "14 I2C0 IER/IDR move the COMP mask",
        check(enabled | disabled == IXR_COMP, || {
            format!("{enabled:#x} {disabled:#x}")
        }),
    ));
    out
}

fn transfer_checks(bus: &mut SystemBus) -> Vec<(&'static str, Res)> {
    let mut out = Vec::new();
    let isr = write_byte(bus, I2C0, ADDR_ABSENT, 0xA5);
    out.push((
        "15 I2C0 write to absent device: NACK status",
        check(isr & IXR_NACK != 0, || format!("{isr:#x}")),
    ));
    wr(bus, I2C0 + ISR, IXR_NACK);
    out.push((
        "16 I2C0 ISR write-1-to-clear (NACK)",
        check(rd(bus, I2C0 + ISR) & IXR_NACK == 0, || "still set".into()),
    ));
    let isr = read_byte(bus, I2C0, ADDR_ABSENT);
    out.push((
        "17 I2C0 read from absent device: NACK status",
        check(isr & IXR_NACK != 0, || format!("{isr:#x}")),
    ));
    out
}

fn second_controller(bus: &mut SystemBus) -> Vec<(&'static str, Res)> {
    let mut out = Vec::new();
    wr(bus, I2C1 + ADDR, 0x33);
    wr(bus, I2C0 + ADDR, ADDR_ABSENT);
    out.push((
        "19 I2C1 ADDR independent of I2C0",
        check(rd(bus, I2C1 + ADDR) & 0x3FF == 0x33, || {
            "I2C0 write reached I2C1".into()
        }),
    ));
    out.push((
        "20 I2C1 TIMEOUT reset",
        check(rd(bus, I2C1 + TIMEOUT) & 0xFF == 0x1F, || "not 0x1F".into()),
    ));
    let isr = write_byte(bus, I2C1, ADDR_ABSENT, 0x5A);
    out.push((
        "21 I2C1 write to absent device: NACK status",
        check(isr & IXR_NACK != 0, || format!("{isr:#x}")),
    ));
    wr(bus, I2C0 + IMR, 0);
    out.push((
        "22 I2C0 IMR is read-only",
        check(rd(bus, I2C0 + IMR) & IXR_ALL == IXR_ALL, || {
            "IMR written".into()
        }),
    ));
    out
}

#[test]
fn conf_iicps_checks_pass_on_the_model() {
    let (mut bus, host) = system();
    host.i2c[0].lock().unwrap().set_peer(Box::new(Eeprom {
        addr: 0x51,
        fill: 0x7E,
    }));
    let mut checks = reset_checks(&mut bus);
    checks.extend(config_checks(&mut bus));
    checks.extend(transfer_checks(&mut bus));
    // Check 18 (N/A in the firmware, no device attached): the EEPROM at 0x51 answers the read.
    let isr = read_byte(&mut bus, I2C0, 0x51);
    let got = rd(&mut bus, I2C0 + DATA);
    checks.push((
        "18 I2C0 read from attached device",
        check(
            isr & (IXR_COMP | IXR_NACK) == IXR_COMP && got == 0x7E,
            || format!("isr {isr:#x} data {got:#x}"),
        ),
    ));
    checks.extend(second_controller(&mut bus));
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
    assert!(failed.is_empty(), "failed: {failed:?}");
    assert_eq!(checks.len(), 22);
}
