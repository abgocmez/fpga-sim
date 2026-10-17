//! The checks of `firmware/conf_gpiops`, encoded as host tests against the GPIO model at
//! 0xE000A000. Names and expected values are the firmware's. Check 12 (pin event interrupt) is
//! N/A in the firmware (no pin source); here the host drives a pin, so it is a PASS: the event
//! reaches the GIC line 52.

mod support;

use mem_sys::SystemBus;
use ps_periph::gpio::{BANK_WIDTH, GPIO_IRQ};
use support::*;

const BASE: u64 = 0xE000_A000;
const MASK_DATA_LSW: u64 = 0x000;
const MASK_DATA_MSW: u64 = 0x004;
const DATA: u64 = 0x040;
const DATA_RO: u64 = 0x060;
const BANK: u64 = 0x204;
const DIRM: u64 = 0x00;
const OEN: u64 = 0x04;
const INT_MASK: u64 = 0x08;
const INT_EN: u64 = 0x0C;
const INT_DIS: u64 = 0x10;
const INT_STAT: u64 = 0x14;
const INT_TYPE: u64 = 0x18;
const INT_POLARITY: u64 = 0x1C;
const INT_ANY: u64 = 0x20;

fn bank(b: u64, off: u64) -> u64 {
    BASE + BANK + 0x40 * b + off
}

type Res = Result<(), String>;

fn check(ok: bool, detail: impl FnOnce() -> String) -> Res {
    if ok { Ok(()) } else { Err(detail()) }
}

/// Each bank in turn; the first failure stops.
fn each_bank(mut f: impl FnMut(u64, u32) -> Res) -> Res {
    for (b, &m) in BANK_WIDTH.iter().enumerate() {
        f(b as u64, m)?;
    }
    Ok(())
}

fn reset_values(bus: &mut SystemBus) -> Res {
    each_bank(|b, m| {
        let dirm = rd(bus, bank(b, DIRM)) & m;
        let oen = rd(bus, bank(b, OEN)) & m;
        let mask = rd(bus, bank(b, INT_MASK)) & m;
        let ty = rd(bus, bank(b, INT_TYPE)) & m;
        check(dirm == 0 && oen == 0 && mask == m && ty == 0, || {
            format!("bank {b}: dirm {dirm:#x} oen {oen:#x} mask {mask:#x} type {ty:#x}")
        })
    })
}

fn dirm_readback(bus: &mut SystemBus) -> Res {
    each_bank(|b, m| {
        for p in [0xA5C3_5A3Cu32, 0x0F0F_F0F0] {
            wr(bus, bank(b, DIRM), p);
            let got = rd(bus, bank(b, DIRM)) & m;
            check(got == p & m, || {
                format!("bank {b}: {got:#x} want {:#x}", p & m)
            })?;
        }
        wr(bus, bank(b, DIRM), 0);
        Ok(())
    })
}

fn oen_readback(bus: &mut SystemBus) -> Res {
    each_bank(|b, m| {
        for p in [0x5A3C_C3A5u32, 0xF00F_0FF0] {
            wr(bus, bank(b, OEN), p);
            let got = rd(bus, bank(b, OEN)) & m;
            check(got == p & m, || {
                format!("bank {b}: {got:#x} want {:#x}", p & m)
            })?;
        }
        wr(bus, bank(b, OEN), 0);
        Ok(())
    })
}

fn data_readback(bus: &mut SystemBus) -> Res {
    each_bank(|b, m| {
        let p = 0x3C5A_A5C3u32;
        wr(bus, BASE + DATA + 4 * b, p);
        let got = rd(bus, BASE + DATA + 4 * b) & m;
        wr(bus, BASE + DATA + 4 * b, 0);
        check(got == p & m, || {
            format!("bank {b}: {got:#x} want {:#x}", p & m)
        })
    })
}

fn masked_write_lsw(bus: &mut SystemBus) -> Res {
    wr(bus, BASE + DATA, 0x5A5A);
    wr(bus, BASE + MASK_DATA_LSW, (0x00FF << 16) | 0x3C00);
    let got = rd(bus, BASE + DATA) & 0xFFFF;
    wr(bus, BASE + DATA, 0);
    check(got == 0x3C5A, || format!("{got:#x}"))
}

fn masked_write_msw(bus: &mut SystemBus) -> Res {
    wr(bus, BASE + DATA + 4 * 3, 0xA5A5_5A5A);
    wr(bus, BASE + MASK_DATA_MSW + 8 * 3, (0x00FF << 16) | 0x3C00);
    let got = rd(bus, BASE + DATA + 4 * 3);
    wr(bus, BASE + DATA + 4 * 3, 0);
    check(got == 0x3CA5_5A5A, || format!("{got:#x}"))
}

fn bank2_width(bus: &mut SystemBus) -> Res {
    wr(bus, BASE + DATA + 4 * 2, 0xFFFF_FFFF);
    let got = rd(bus, BASE + DATA + 4 * 2);
    wr(bus, BASE + DATA + 4 * 2, 0);
    check(got == BANK_WIDTH[2], || format!("{got:#x}"))
}

fn int_mask_en_dis(bus: &mut SystemBus) -> Res {
    let b = 1;
    wr(bus, bank(b, INT_DIS), 0xFFFF);
    let all = rd(bus, bank(b, INT_MASK)) & 0xFFFF;
    check(all == 0xFFFF, || format!("after DIS {all:#x}"))?;
    wr(bus, bank(b, INT_EN), 0x0005);
    let some = rd(bus, bank(b, INT_MASK)) & 0xFFFF;
    check(some == 0xFFFA, || format!("after EN {some:#x}"))?;
    wr(bus, bank(b, INT_DIS), 0x0001);
    let last = rd(bus, bank(b, INT_MASK)) & 0xFFFF;
    check(last == 0xFFFB, || format!("after DIS {last:#x}"))
}

fn int_type_polarity_any(bus: &mut SystemBus) -> Res {
    let b = 2;
    let m = BANK_WIDTH[2];
    wr(bus, bank(b, INT_TYPE), 0x0F0F_0F0F);
    wr(bus, bank(b, INT_POLARITY), 0x0000_F0F3);
    wr(bus, bank(b, INT_ANY), 0x0000_0F05);
    let ty = rd(bus, bank(b, INT_TYPE)) & m;
    let pol = rd(bus, bank(b, INT_POLARITY)) & m;
    let any = rd(bus, bank(b, INT_ANY)) & m;
    for x in [bank(b, INT_TYPE), bank(b, INT_POLARITY), bank(b, INT_ANY)] {
        wr(bus, x, 0);
    }
    check(ty == 0x0F0F_0F0F & m, || format!("type {ty:#x}"))?;
    check(pol == 0x0000_F0F3 & m, || format!("polarity {pol:#x}"))?;
    check(any == 0x0000_0F05 & m, || format!("any {any:#x}"))
}

fn int_status_no_events(bus: &mut SystemBus) -> Res {
    wr(bus, bank(1, INT_STAT), 0xFFFF);
    let got = rd(bus, bank(1, INT_STAT)) & 0xFFFF;
    check(got == 0, || format!("{got:#x}"))
}

fn data_ro_mirrors_output(bus: &mut SystemBus) -> Res {
    wr(bus, bank(0, DIRM), 0x00FF);
    wr(bus, bank(0, OEN), 0x00FF);
    wr(bus, BASE + DATA, 0x00A5);
    let got = rd(bus, BASE + DATA_RO) & 0x00FF;
    wr(bus, bank(0, DIRM), 0);
    wr(bus, bank(0, OEN), 0);
    wr(bus, BASE + DATA, 0);
    check(got == 0x00A5, || format!("{got:#x}"))
}

/// Pin 20 is driven by the host; the event reaches the GIC as SPI 52 and is acknowledged.
fn pin_event_interrupt(bus: &mut SystemBus, host: &ps_periph::PsHandles) -> Res {
    gic_up(bus);
    gic_enable_spi(bus, GPIO_IRQ.into());
    // Bank 1, pin 20 (MIO 20), edge type, rising edge selected, interrupt enabled.
    wr(bus, bank(1, INT_TYPE), 1 << 4);
    wr(bus, bank(1, INT_POLARITY), 1 << 4);
    wr(bus, bank(1, INT_EN), 1 << 4);
    host.gpio.lock().unwrap().set_input(20, true);
    tick_us(bus, 1);
    let stat = rd(bus, bank(1, INT_STAT));
    check(stat == 1 << 4, || format!("status {stat:#x}"))?;
    check(bus.core_irq(0), || "GIC line 52 not asserted".to_string())?;
    let taken = service_irqs(bus, |b, id| {
        if id == 52 {
            wr(b, bank(1, INT_STAT), 1 << 4);
        }
    });
    check(taken == vec![52], || format!("taken {taken:?}"))
}

#[test]
fn conf_gpiops_checks_pass_on_the_model() {
    let (mut bus, host) = system();
    assert_eq!(GPIO_IRQ, 52);
    let checks: Vec<(&str, Res)> = vec![
        ("reset_values", reset_values(&mut bus)),
        ("dirm_readback", dirm_readback(&mut bus)),
        ("oen_readback", oen_readback(&mut bus)),
        ("data_readback", data_readback(&mut bus)),
        ("masked_write_lsw", masked_write_lsw(&mut bus)),
        ("masked_write_msw", masked_write_msw(&mut bus)),
        ("bank2_width", bank2_width(&mut bus)),
        ("int_mask_en_dis", int_mask_en_dis(&mut bus)),
        ("int_type_polarity_any", int_type_polarity_any(&mut bus)),
        ("int_status_no_events", int_status_no_events(&mut bus)),
        ("data_ro_mirrors_output", data_ro_mirrors_output(&mut bus)),
        ("pin_event_interrupt", pin_event_interrupt(&mut bus, &host)),
    ];
    let mut failed = Vec::new();
    for (n, (name, res)) in checks.iter().enumerate() {
        match res {
            Ok(()) => println!("CHECK {} {name}: PASS", n + 1),
            Err(why) => {
                println!("CHECK {} {name}: FAIL {why}", n + 1);
                failed.push(*name);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}
