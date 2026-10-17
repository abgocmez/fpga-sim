//! The checks of `firmware/conf_spips`, encoded as host tests against the SPI models at
//! 0xE0006000 (SPI0, GIC 58) and 0xE0007000 (SPI1, GIC 81). Names and expected values are the
//! firmware's. Where the firmware reports N/A because QEMU maps nothing, the model answers and
//! the check is run: the loopback check (firmware check 23) is PASS with no slave (MISO pulled
//! high, 0xFF) and again with the host loopback peer; the interrupt line (firmware check 27) is
//! PASS on GIC 58.

mod support;

use mem_sys::SystemBus;
use ps_periph::spi::{Loopback, SPI_IRQS};
use support::*;

const SPI0: u64 = 0xE000_6000;
const SPI1: u64 = 0xE000_7000;
const CR: u64 = 0x00;
const ISR: u64 = 0x04;
const IER: u64 = 0x08;
const IDR: u64 = 0x0C;
const IMR: u64 = 0x10;
const ER: u64 = 0x14;
const DR: u64 = 0x18;
const TXD: u64 = 0x1C;
const RXD: u64 = 0x20;
const SICR: u64 = 0x24;
const TXWR: u64 = 0x28;
const RXWR: u64 = 0x2C;

const CR_MSTREN: u32 = 1 << 0;
const CR_CPHA: u32 = 1 << 2;
const CR_BAUD_8: u32 = 0b011 << 3;
const CR_MANSTRT: u32 = 1 << 15;
const CR_MANSTRTEN: u32 = 1 << 16;
const CR_SSFORCE: u32 = 1 << 14;
const CR_CS_MASK: u32 = 0xF << 10;
const CR_CS0: u32 = 0b1110 << 10;
const CR_MODEFAIL_EN: u32 = 1 << 17;
const CR_RESET: u32 = CR_MODEFAIL_EN;
const IXR_TXNFULL: u32 = 1 << 2;
const IXR_TXFULL: u32 = 1 << 3;
const IXR_ALL: u32 = 0x7F;
const FIFO_DEPTH: u32 = 128;

type Res = Result<(), String>;

fn check(ok: bool, detail: impl FnOnce() -> String) -> Res {
    if ok { Ok(()) } else { Err(detail()) }
}

fn is_live(bus: &mut SystemBus, base: u64) -> bool {
    wr(bus, base + DR, 0x1234);
    let ok = rd(bus, base + DR) == 0x1234;
    wr(bus, base + DR, 0);
    ok
}

/// The reset-state and configuration checks on SPI0 (firmware checks 1 to 23).
fn spi0_checks(bus: &mut SystemBus) -> Vec<(&'static str, Res)> {
    let mut out: Vec<(&'static str, Res)> = Vec::new();
    let mut push = |name: &'static str, r: Res| out.push((name, r));
    push(
        "1 SPI0 Config reset",
        check(rd(bus, SPI0 + CR) == CR_RESET, || {
            format!("{:#x}", rd(bus, SPI0 + CR))
        }),
    );
    push(
        "2 SPI0 Enable reset",
        check(rd(bus, SPI0 + ER) & 1 == 0, || "enabled".into()),
    );
    push(
        "3 SPI0 Delay reset",
        check(rd(bus, SPI0 + DR) == 0, || "nonzero".into()),
    );
    push(
        "4 SPI0 TX threshold reset",
        check(rd(bus, SPI0 + TXWR) & 0xFF == 1, || {
            format!("{}", rd(bus, SPI0 + TXWR))
        }),
    );
    push(
        "5 SPI0 RX threshold reset",
        check(rd(bus, SPI0 + RXWR) & 0xFF == 1, || {
            format!("{}", rd(bus, SPI0 + RXWR))
        }),
    );
    push(
        "6 SPI0 Slave_Idle_count reset",
        check(rd(bus, SPI0 + SICR) & 0xFF == 0, || "nonzero".into()),
    );
    let isr = rd(bus, SPI0 + ISR);
    push(
        "7 SPI0 empty TX FIFO: not full",
        check(isr & IXR_TXNFULL != 0, || format!("{isr:#x}")),
    );
    push(
        "8 SPI0 empty TX FIFO: full clear",
        check(isr & IXR_TXFULL == 0, || format!("{isr:#x}")),
    );
    push(
        "9 SPI0 interrupt mask reset (all masked)",
        check(rd(bus, SPI0 + IMR) & IXR_ALL == IXR_ALL, || {
            format!("{:#x}", rd(bus, SPI0 + IMR))
        }),
    );

    wr(bus, SPI0 + CR, CR_RESET | CR_MSTREN | CR_CPHA | CR_BAUD_8);
    let got = rd(bus, SPI0 + CR) & 0x3F;
    push(
        "10 SPI0 Config write/read",
        check(got == CR_MSTREN | CR_CPHA | CR_BAUD_8, || {
            format!("{got:#x}")
        }),
    );
    let cr = rd(bus, SPI0 + CR) | CR_MANSTRTEN | CR_SSFORCE | CR_CS0;
    wr(bus, SPI0 + CR, cr);
    let got = rd(bus, SPI0 + CR) & (CR_MANSTRTEN | CR_SSFORCE | CR_CS_MASK);
    push(
        "11 SPI0 manual start enable, SS force and CS0",
        check(got == CR_MANSTRTEN | CR_SSFORCE | CR_CS0, || {
            format!("{got:#x}")
        }),
    );

    wr(bus, SPI0 + DR, 0x0102_0304);
    push(
        "12 SPI0 Delay write/read",
        check(rd(bus, SPI0 + DR) == 0x0102_0304, || "mismatch".into()),
    );
    wr(bus, SPI0 + TXWR, 0x10);
    push(
        "13 SPI0 TX threshold",
        check(rd(bus, SPI0 + TXWR) & 0xFF == 0x10, || "mismatch".into()),
    );
    wr(bus, SPI0 + RXWR, 0x08);
    push(
        "14 SPI0 RX threshold",
        check(rd(bus, SPI0 + RXWR) & 0xFF == 0x08, || "mismatch".into()),
    );
    wr(bus, SPI0 + SICR, 0x0A);
    push(
        "15 SPI0 Slave_Idle_count",
        check(rd(bus, SPI0 + SICR) & 0xFF == 0x0A, || "mismatch".into()),
    );

    wr(bus, SPI0 + IER, IXR_TXNFULL);
    let enabled = rd(bus, SPI0 + IMR) & IXR_TXNFULL;
    wr(bus, SPI0 + IDR, IXR_TXNFULL);
    let disabled = rd(bus, SPI0 + IMR) & IXR_TXNFULL;
    push(
        "16 SPI0 IER/IDR move the TX_FIFO_not_full mask",
        check(enabled | disabled == IXR_TXNFULL, || {
            format!("{enabled:#x} {disabled:#x}")
        }),
    );

    wr(bus, SPI0 + ER, 1);
    push(
        "17 SPI0 Enable write/read (1)",
        check(rd(bus, SPI0 + ER) & 1 == 1, || "0".into()),
    );
    wr(bus, SPI0 + ER, 0);
    push(
        "18 SPI0 Enable write/read (0)",
        check(rd(bus, SPI0 + ER) & 1 == 0, || "1".into()),
    );
    out
}

/// Firmware checks 19 to 27: FIFO fill, manual transfer, SPI1 independence, interrupt line.
fn spi0_fifo_and_transfer(
    bus: &mut SystemBus,
    host: &ps_periph::PsHandles,
) -> Vec<(&'static str, Res)> {
    let mut out: Vec<(&'static str, Res)> = Vec::new();
    wr(bus, SPI0 + ISR, IXR_ALL);
    wr(
        bus,
        SPI0 + CR,
        CR_RESET | CR_MSTREN | CR_CPHA | CR_BAUD_8 | CR_MANSTRTEN | CR_SSFORCE | CR_CS0,
    );
    for i in 0..FIFO_DEPTH {
        wr(bus, SPI0 + TXD, i);
    }
    let isr = rd(bus, SPI0 + ISR);
    out.push((
        "19 SPI0 TX FIFO full after 128 writes: TX_FIFO_full set",
        check(isr & IXR_TXFULL != 0, || format!("{isr:#x}")),
    ));
    out.push((
        "20 SPI0 full TX FIFO: TX_FIFO_not_full clear",
        check(isr & IXR_TXNFULL == 0, || format!("{isr:#x}")),
    ));

    host.spi[0].lock().unwrap().set_peer(Box::new(Loopback));
    wr(bus, SPI0 + ER, 1);
    let cr = rd(bus, SPI0 + CR);
    wr(bus, SPI0 + CR, cr | CR_MANSTRT);
    let isr = rd(bus, SPI0 + ISR);
    out.push((
        "21 SPI0 manual start drains the TX FIFO",
        check(isr & IXR_TXFULL == 0, || format!("{isr:#x}")),
    ));
    out.push((
        "22 SPI0 RX FIFO not empty after the transfer",
        check(isr & 0x10 != 0, || format!("{isr:#x}")),
    ));
    // Firmware check 23: the first byte through the bus. The firmware does not assert it. Here
    // the loopback peer returns the byte sent (0x00 for the first word), and with no slave the
    // answer is 0xFF; both are checked.
    let first = rd(bus, SPI0 + RXD) & 0xFF;
    out.push((
        "23 SPI0 loopback of the first byte",
        check(first == 0, || {
            format!("first byte {first:#x}, want the first word sent (0x00)")
        }),
    ));
    // Drain the rest of the loopback words, then one byte with no slave attached.
    for _ in 0..(FIFO_DEPTH - 1) {
        rd(bus, SPI0 + RXD);
    }
    host.spi[0]
        .lock()
        .unwrap()
        .set_peer(Box::new(ps_periph::NoSlave));
    wr(bus, SPI0 + TXD, 0x55);
    let cr = rd(bus, SPI0 + CR);
    wr(bus, SPI0 + CR, cr | CR_MANSTRT);
    let no_slave = rd(bus, SPI0 + RXD) & 0xFF;
    out.push((
        "23b SPI0 with no slave the answer is pulled high",
        check(no_slave == 0xFF, || format!("{no_slave:#x}")),
    ));

    wr(bus, SPI1 + DR, 0x5566_7788);
    out.push((
        "24 SPI0 Delay unchanged by a write to SPI1",
        check(rd(bus, SPI0 + DR) == 0x0102_0304, || {
            format!("{:#x}", rd(bus, SPI0 + DR))
        }),
    ));
    out.push((
        "25 SPI1 Delay write/read",
        check(rd(bus, SPI1 + DR) == 0x5566_7788, || {
            format!("{:#x}", rd(bus, SPI1 + DR))
        }),
    ));
    out.push((
        "26 SPI1 Config reset",
        check(
            rd(bus, SPI1 + CR) & CR_MODEFAIL_EN == CR_MODEFAIL_EN,
            || format!("{:#x}", rd(bus, SPI1 + CR)),
        ),
    ));

    // Firmware check 27 is N/A there (status only). Here the line is checked: the TX FIFO
    // not-full interrupt is enabled and the level is asserted on GIC 58.
    wr(bus, SPI0 + IER, IXR_TXNFULL);
    wr(bus, SPI0 + ISR, IXR_TXNFULL);
    let mut lines = Vec::new();
    mem_sys::Device::irq_lines(&*host.spi[0].lock().unwrap(), &mut lines);
    out.push((
        "27 SPI0 interrupt line to the GIC (58)",
        check(
            SPI_IRQS[0] == 58 && lines[0].id == 58 && lines[0].level,
            || format!("{lines:?}"),
        ),
    ));
    out
}

#[test]
fn conf_spips_checks_pass_on_the_model() {
    let (mut bus, host) = system();
    assert!(is_live(&mut bus, SPI0) && is_live(&mut bus, SPI1));
    let mut checks = spi0_checks(&mut bus);
    checks.extend(spi0_fifo_and_transfer(&mut bus, &host));
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
    assert_eq!(checks.len(), 28);
}
