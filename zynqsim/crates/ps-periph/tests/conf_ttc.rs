//! The checks of `firmware/conf_ttc`, encoded as host tests against the bus model: TTC0 at
//! 0xF8001000 and TTC1 at 0xF8002000, three counters each, with the interval interrupt
//! through the GIC (SPI 42 and 69).
//!
//! Check 12 (the match flag stays set across reads) is a PASS here: the model clears the flag
//! only on a write of 1, as UG585 describes. The QEMU model clears it on read; the firmware
//! reports that as N/A, and this test does not reproduce it.

mod support;

use mem_sys::SystemBus;
use ps_periph::ttc::TTC_IRQS;
use support::*;

const TTC: [u64; 2] = [0xF800_1000, 0xF800_2000];
const CLK_CTRL: u64 = 0x00;
const CNT_CTRL: u64 = 0x0C;
const CNT_VAL: u64 = 0x18;
const INTERVAL: u64 = 0x24;
const ISR: u64 = 0x54;
const IER: u64 = 0x60;

const CC_DIS: u32 = 1 << 0;
const CC_INTERVAL: u32 = 1 << 1;
const CC_DECR: u32 = 1 << 2;
const CC_MATCH: u32 = 1 << 3;
const CC_RST: u32 = 1 << 4;
const ISR_INTERVAL: u32 = 1 << 0;
const ISR_MATCH1: u32 = 1 << 2;

fn reg(t: usize, n: u64, off: u64) -> u64 {
    TTC[t] + off + 4 * n
}

fn match_reg(t: usize, n: u64, x: u64) -> u64 {
    TTC[t] + 0x30 + 0x0C * n + 4 * x
}

fn stop(bus: &mut SystemBus, t: usize, n: u64) {
    wr(bus, reg(t, n, CNT_CTRL), CC_DIS);
    wr(bus, reg(t, n, IER), 0);
    wr(bus, reg(t, n, ISR), 0x1F);
}

type Res = Result<(), String>;
type Check = (&'static str, fn(&mut SystemBus) -> Res);

fn check(ok: bool, detail: impl FnOnce() -> String) -> Res {
    if ok { Ok(()) } else { Err(detail()) }
}

fn counter_reset_state(bus: &mut SystemBus) -> Res {
    let cc = rd(bus, reg(0, 0, CNT_CTRL));
    check(cc & CC_DIS != 0, || format!("CNT_CTRL=0x{cc:x}"))
}

fn clock_control_readback(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 0, CLK_CTRL), 1 | (3 << 1));
    let clk = rd(bus, reg(0, 0, CLK_CTRL)) & 0x47;
    wr(bus, reg(0, 0, CLK_CTRL), 0);
    check(clk == 0x07, || format!("got 0x{clk:x} want 0x07"))
}

fn interval_readback(bus: &mut SystemBus) -> Res {
    let iv = [0xA5A5u32, 0x1234, 0x0F0F];
    for (n, v) in iv.iter().enumerate() {
        wr(bus, reg(0, n as u64, INTERVAL), *v);
    }
    let got: Vec<u32> = (0..3)
        .map(|n| rd(bus, reg(0, n, INTERVAL)) & 0xFFFF)
        .collect();
    check(got == iv, || format!("got {got:x?} want {iv:x?}"))
}

fn match_readback(bus: &mut SystemBus) -> Res {
    let mut mism = 0;
    for n in 0..3u64 {
        for x in 0..3u64 {
            let want = 0x100 * (n as u32 * 3 + x as u32 + 1) + 0x7;
            wr(bus, match_reg(0, n, x), want);
            if rd(bus, match_reg(0, n, x)) & 0xFFFF != want {
                mism += 1;
            }
        }
    }
    check(mism == 0, || format!("{mism} of 9 differ"))
}

fn ttc1_separate(bus: &mut SystemBus) -> Res {
    wr(bus, reg(1, 0, INTERVAL), 0x3456);
    let t0 = rd(bus, reg(0, 0, INTERVAL)) & 0xFFFF;
    let t1 = rd(bus, reg(1, 0, INTERVAL)) & 0xFFFF;
    check(t0 == 0xA5A5 && t1 == 0x3456, || {
        format!("TTC0 0x{t0:x} TTC1 0x{t1:x}")
    })
}

fn counter_reset_clears_value(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 0, INTERVAL), 0xFFFF);
    wr(bus, reg(0, 0, CNT_CTRL), 0);
    tick_us(bus, 500);
    let before = rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF;
    wr(bus, reg(0, 0, CNT_CTRL), CC_DIS | CC_RST);
    let after = rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF;
    let rst_bit = rd(bus, reg(0, 0, CNT_CTRL)) & CC_RST;
    stop(bus, 0, 0);
    check(before != 0 && after == 0 && rst_bit == 0, || {
        format!(
            "before {before} after {after} RST still set {}",
            rst_bit != 0
        )
    })
}

fn overflow_mode_counts_past_interval(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 0, INTERVAL), 1000);
    wr(bus, reg(0, 0, ISR), 0x1F);
    wr(bus, reg(0, 0, CNT_CTRL), 0);
    let mut max_seen = 0;
    for _ in 0..5 {
        tick_us(bus, 200);
        max_seen = max_seen.max(rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF);
    }
    stop(bus, 0, 0);
    check(max_seen > 1000, || {
        format!("largest Counter_Value {max_seen}")
    })
}

fn interval_mode_wraps(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 0, INTERVAL), 1000);
    wr(bus, reg(0, 0, ISR), 0x1F);
    wr(bus, reg(0, 0, CNT_CTRL), CC_INTERVAL);
    tick_us(bus, 2000);
    let isr = rd(bus, reg(0, 0, ISR));
    let v = rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF;
    stop(bus, 0, 0);
    check(isr & ISR_INTERVAL != 0 && v < 1000, || {
        format!("ISR=0x{isr:x} Counter_Value {v}")
    })
}

fn decrement_counts_down(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 0, INTERVAL), 0xFFFF);
    wr(bus, reg(0, 0, CNT_CTRL), CC_DECR);
    tick_us(bus, 10);
    let v1 = rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF;
    tick_us(bus, 20);
    let v2 = rd(bus, reg(0, 0, CNT_VAL)) & 0xFFFF;
    stop(bus, 0, 0);
    check(v2 < v1, || format!("first {v1} then {v2}"))
}

fn match1_sets_flag(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 1, INTERVAL), 0xFFFF);
    wr(bus, match_reg(0, 1, 0), 300);
    wr(bus, reg(0, 1, ISR), 0x1F);
    wr(bus, reg(0, 1, CNT_CTRL), CC_MATCH);
    let mut isr_m = 0;
    for _ in 0..3000 {
        isr_m = rd(bus, reg(0, 1, ISR));
        if isr_m & ISR_MATCH1 != 0 {
            break;
        }
        tick_us(bus, 10);
    }
    let vm = rd(bus, reg(0, 1, CNT_VAL)) & 0xFFFF;
    stop(bus, 0, 1);
    check(isr_m & ISR_MATCH1 != 0 && vm >= 300, || {
        format!("ISR=0x{isr_m:x} Counter_Value {vm}")
    })
}

fn prescaler_divides_by_16(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 2, INTERVAL), 0xFFFF);
    wr(bus, reg(0, 2, CLK_CTRL), 0);
    wr(bus, reg(0, 2, CNT_CTRL), 0);
    tick_us(bus, 200);
    let plain = rd(bus, reg(0, 2, CNT_VAL)) & 0xFFFF;
    wr(bus, reg(0, 2, CNT_CTRL), CC_DIS | CC_RST);
    wr(bus, reg(0, 2, CLK_CTRL), 1 | (3 << 1));
    wr(bus, reg(0, 2, CNT_CTRL), 0);
    tick_us(bus, 200);
    let scaled = rd(bus, reg(0, 2, CNT_VAL)) & 0xFFFF;
    stop(bus, 0, 2);
    wr(bus, reg(0, 2, CLK_CTRL), 0);
    check(
        scaled != 0 && plain >= 8 * scaled && plain <= 32 * scaled,
        || format!("plain {plain} prescaled(/16) {scaled}"),
    )
}

fn match_flag_is_sticky(bus: &mut SystemBus) -> Res {
    wr(bus, reg(0, 1, INTERVAL), 0xFFFF);
    wr(bus, match_reg(0, 1, 0), 300);
    wr(bus, reg(0, 1, ISR), 0x1F);
    wr(bus, reg(0, 1, CNT_CTRL), CC_MATCH);
    tick_us(bus, 1000);
    wr(bus, reg(0, 1, CNT_CTRL), CC_DIS);
    let r1 = rd(bus, reg(0, 1, ISR)) & ISR_MATCH1;
    let r2 = rd(bus, reg(0, 1, ISR)) & ISR_MATCH1;
    stop(bus, 0, 1);
    check(r1 != 0 && r2 != 0, || format!("reads 0x{r1:x} 0x{r2:x}"))
}

/// The interval interrupt of counter 0 of each block, through the GIC. Returns the number of
/// times each block's handler ran.
fn interval_irq_through_gic(bus: &mut SystemBus, t: usize) -> Result<u32, String> {
    gic_enable_spi(bus, TTC_IRQS[t][0] as u32);
    let mut count = [0u32; 2];
    wr(bus, reg(t, 0, INTERVAL), 1000);
    wr(bus, reg(t, 0, ISR), 0x1F);
    wr(bus, reg(t, 0, IER), ISR_INTERVAL);
    wr(bus, reg(t, 0, CNT_CTRL), CC_INTERVAL);
    let mut waited = 0;
    while count[t] < 3 && waited < 5000 {
        tick_us(bus, 20);
        service_irqs(bus, |b, id| {
            for (blk, ids) in TTC_IRQS.iter().enumerate() {
                if id == u32::from(ids[0]) && rd(b, reg(blk, 0, ISR)) & ISR_INTERVAL != 0 {
                    wr(b, reg(blk, 0, ISR), ISR_INTERVAL);
                    count[blk] += 1;
                }
            }
        });
        waited += 1;
    }
    stop(bus, t, 0);
    Ok(count[t])
}

#[test]
fn conf_ttc_every_check_passes() {
    let (mut bus, _h) = system();
    println!("conf_ttc: xttcps conformance, TTC0 0xF8001000, TTC1 0xF8002000");
    for t in 0..2 {
        for n in 0..3 {
            stop(&mut bus, t, n);
        }
    }
    gic_up(&mut bus);
    let checks: Vec<Check> = vec![
        (
            "Counter_Control reset: counter disabled",
            counter_reset_state,
        ),
        (
            "Clock_Control readback (PS_EN, PS_VAL)",
            clock_control_readback,
        ),
        ("Interval_Value readback (3 counters)", interval_readback),
        ("Match_1..3 readback (3 counters)", match_readback),
        (
            "TTC1 registers separate from TTC0 (0xF8002000)",
            ttc1_separate,
        ),
        (
            "Counter reset clears Counter_Value",
            counter_reset_clears_value,
        ),
        (
            "Overflow mode counts past Interval_Value",
            overflow_mode_counts_past_interval,
        ),
        (
            "Interval mode wraps and sets the interval flag",
            interval_mode_wraps,
        ),
        ("Decrement mode counts down", decrement_counts_down),
        ("Match_1 sets the match flag", match1_sets_flag),
        (
            "Prescaler divides the clock by 2^(PS_VAL+1)",
            prescaler_divides_by_16,
        ),
        (
            "Match flag sticky across reads (cleared only by writing 1)",
            match_flag_is_sticky,
        ),
    ];
    let mut failures = Vec::new();
    for (n, (name, f)) in checks.iter().enumerate() {
        match f(&mut bus) {
            Ok(()) => println!("CHECK {} {name}: PASS", n + 1),
            Err(why) => {
                println!("CHECK {} {name}: FAIL {why}", n + 1);
                failures.push(*name);
            }
        }
    }
    for (t, num) in [(0usize, 13u32), (1, 14)] {
        let name = if t == 0 {
            "Interval interrupt reaches GIC (TTC0 counter 0, ID 42)"
        } else {
            "Interval interrupt reaches GIC (TTC1 counter 0, ID 69)"
        };
        let handled = interval_irq_through_gic(&mut bus, t).unwrap();
        if handled >= 3 {
            println!("CHECK {num} {name}: PASS");
        } else {
            println!("CHECK {num} {name}: FAIL handled {handled} times");
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
    println!("RESULT: ALL PASS");
}
