//! The checks of `firmware/conf_scutimer`, encoded as host tests against the bus model.
//!
//! The firmware runs on the Xilinx QEMU, where four global-timer checks fail; this model
//! follows the TRM there, so every check here is expected to PASS. Same numbering and names
//! as the firmware's `CHECK n name` lines. Run with `--nocapture` to see them.

mod support;

use mem_sys::SystemBus;
use ps_periph::timers::{GLOBAL_TIMER_PPI, PRIVATE_TIMER_PPI};
use support::*;

const PT: u64 = 0xF8F0_0600;
const PT_LOAD: u64 = PT;
const PT_COUNTER: u64 = PT + 4;
const PT_CONTROL: u64 = PT + 8;
const PT_ISR: u64 = PT + 0xC;
const GT: u64 = 0xF8F0_0200;
const GT_CNT_LO: u64 = GT;
const GT_CNT_HI: u64 = GT + 4;
const GT_CONTROL: u64 = GT + 8;
const GT_ISR: u64 = GT + 0xC;
const GT_CMP_LO: u64 = GT + 0x10;
const GT_CMP_HI: u64 = GT + 0x14;
const GT_INCR: u64 = GT + 0x18;
const PERIOD: u32 = 0x4000;

fn pt_stop(bus: &mut SystemBus) {
    wr(bus, PT_CONTROL, 0);
    wr(bus, PT_ISR, 1);
}

fn gt_stop(bus: &mut SystemBus) {
    wr(bus, GT_CONTROL, 0);
    wr(bus, GT_ISR, 1);
}

/// One check result: `Ok` for PASS, `Err` with the got/want text for FAIL.
type Res = Result<(), String>;
type Check = (&'static str, fn(&mut SystemBus) -> Res);

fn cmp(got: u32, want: u32) -> Res {
    if got == want {
        Ok(())
    } else {
        Err(format!("got=0x{got:08x} want=0x{want:08x}"))
    }
}

fn reset_state(bus: &mut SystemBus) -> Res {
    if rd(bus, PT_CONTROL) & 0xFF07 != 0 {
        return Err("private timer runs after reset".into());
    }
    if rd(bus, PT_ISR) & 1 != 0 {
        return Err("private timer flag set after reset".into());
    }
    cmp(rd(bus, GT_CONTROL) & 0xFF0F, 0)
}

fn pt_load_readback(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, 0x1234_5678);
    let r = cmp(rd(bus, PT_LOAD), 0x1234_5678);
    wr(bus, PT_LOAD, 0);
    r
}

fn pt_counter_writable_when_stopped(bus: &mut SystemBus) -> Res {
    wr(bus, PT_CONTROL, 0);
    wr(bus, PT_COUNTER, 0xBEE);
    let r = cmp(rd(bus, PT_COUNTER), 0xBEE);
    wr(bus, PT_COUNTER, 0);
    r
}

fn pt_control_readback(bus: &mut SystemBus) -> Res {
    wr(bus, PT_CONTROL, 0xFF04);
    let r = cmp(rd(bus, PT_CONTROL) & 0xFF07, 0xFF04);
    wr(bus, PT_CONTROL, 0);
    r
}

fn pt_counts_down_and_stops(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, 0xFFFF_FFFF);
    wr(bus, PT_COUNTER, 0xFFFF_FFFF);
    wr(bus, PT_CONTROL, 1);
    tick_us(bus, 100);
    let c1 = rd(bus, PT_COUNTER);
    tick_us(bus, 100);
    let c2 = rd(bus, PT_COUNTER);
    wr(bus, PT_CONTROL, 0);
    if c2 >= c1 {
        return Err(format!("c1=0x{c1:08x} c2=0x{c2:08x}: did not count down"));
    }
    let stopped = rd(bus, PT_COUNTER);
    tick_us(bus, 100);
    cmp(rd(bus, PT_COUNTER), stopped)
}

fn pt_event_flag_w1c(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, PERIOD);
    wr(bus, PT_COUNTER, PERIOD);
    wr(bus, PT_CONTROL, 1);
    let set = wait_until(bus, 2000, |b| rd(b, PT_ISR) & 1 != 0);
    wr(bus, PT_ISR, 1);
    let after = rd(bus, PT_ISR) & 1;
    wr(bus, PT_CONTROL, 0);
    if !set {
        return Err("flag never set".into());
    }
    cmp(after, 0)
}

fn pt_auto_reload(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, PERIOD);
    wr(bus, PT_COUNTER, PERIOD);
    wr(bus, PT_CONTROL, 1 | 2);
    let mut events = 0;
    for _ in 0..5000 {
        if rd(bus, PT_ISR) & 1 != 0 {
            wr(bus, PT_ISR, 1);
            events += 1;
            if events == 3 {
                break;
            }
        }
        tick_us(bus, 10);
    }
    let counter = rd(bus, PT_COUNTER);
    pt_stop(bus);
    if events != 3 {
        return Err(format!("got={events} want=3"));
    }
    if counter <= PERIOD {
        Ok(())
    } else {
        Err(format!("counter 0x{counter:x} above LOAD"))
    }
}

fn pt_one_shot_stops(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, PERIOD);
    wr(bus, PT_COUNTER, PERIOD);
    wr(bus, PT_CONTROL, 1);
    let set = wait_until(bus, 2000, |b| rd(b, PT_ISR) & 1 != 0);
    wr(bus, PT_ISR, 1);
    if !set {
        pt_stop(bus);
        return Err("flag never set".into());
    }
    let held = rd(bus, PT_COUNTER);
    tick_us(bus, 300);
    let again = rd(bus, PT_ISR) & 1;
    let now = rd(bus, PT_COUNTER);
    pt_stop(bus);
    if again != 0 {
        return Err("a second event after one-shot".into());
    }
    cmp(now, held)
}

fn pt_prescaler_slows(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, 0xFFFF_FFFF);
    wr(bus, PT_COUNTER, 0xFFFF_FFFF);
    wr(bus, PT_CONTROL, 1);
    tick_us(bus, 100);
    let fast = 0xFFFF_FFFFu32 - rd(bus, PT_COUNTER);
    pt_stop(bus);
    wr(bus, PT_COUNTER, 0xFFFF_FFFF);
    wr(bus, PT_CONTROL, 1 | (0xFF << 8));
    tick_us(bus, 100);
    let slow = 0xFFFF_FFFFu32 - rd(bus, PT_COUNTER);
    pt_stop(bus);
    if fast > 16 * slow {
        Ok(())
    } else {
        Err(format!("fast {fast} slow {slow}"))
    }
}

fn pt_flag_without_irq(bus: &mut SystemBus) -> Res {
    wr(bus, PT_LOAD, PERIOD);
    wr(bus, PT_COUNTER, PERIOD);
    wr(bus, PT_CONTROL, 1);
    let set = wait_until(bus, 2000, |b| rd(b, PT_ISR) & 1 != 0);
    pt_stop(bus);
    tick_us(bus, 50);
    if !set {
        return Err("flag never set".into());
    }
    cmp(u32::from(bus.core_irq(0)), 0)
}

/// Core 0's interrupt handler for the two timers: clears the flag the interrupt came from.
fn timer_handler(bus: &mut SystemBus, id: u32, taken: &mut [u32; 2]) {
    if id == u32::from(PRIVATE_TIMER_PPI) {
        if rd(bus, PT_ISR) & 1 != 0 {
            wr(bus, PT_ISR, 1);
            taken[0] += 1;
        }
    } else if id == u32::from(GLOBAL_TIMER_PPI) && rd(bus, GT_ISR) & 1 != 0 {
        wr(bus, GT_ISR, 1);
        let control = rd(bus, GT_CONTROL);
        wr(bus, GT_CONTROL, control & !0x4);
        taken[1] += 1;
    }
}

fn pt_irq_gic_ppi29(bus: &mut SystemBus) -> Res {
    gic_up(bus);
    gic_enable_ppi(bus, u32::from(PRIVATE_TIMER_PPI));
    let mut taken = [0u32; 2];
    wr(bus, PT_LOAD, PERIOD);
    wr(bus, PT_COUNTER, PERIOD);
    wr(bus, PT_CONTROL, 1 | 2 | 4);
    for _ in 0..5000 {
        service_irqs(bus, |b, id| timer_handler(b, id, &mut taken));
        if taken[0] >= 3 {
            break;
        }
        tick_us(bus, 10);
    }
    pt_stop(bus);
    if taken[0] >= 3 {
        Ok(())
    } else {
        Err(format!("handled {} times, want >= 3", taken[0]))
    }
}

fn gt_counter_advances(bus: &mut SystemBus) -> Res {
    wr(bus, GT_CONTROL, 1);
    let c1 = rd(bus, GT_CNT_LO);
    tick_us(bus, 100);
    let c2 = rd(bus, GT_CNT_LO);
    gt_stop(bus);
    if c2 > c1 {
        Ok(())
    } else {
        Err(format!("c1 {c1} c2 {c2}"))
    }
}

fn gt_counter_writable_when_stopped(bus: &mut SystemBus) -> Res {
    wr(bus, GT_CONTROL, 0);
    wr(bus, GT_CNT_HI, 0);
    wr(bus, GT_CNT_LO, 0xBEE0);
    cmp(rd(bus, GT_CNT_LO), 0xBEE0)
}

fn gt_counter_holds_when_disabled(bus: &mut SystemBus) -> Res {
    wr(bus, GT_CONTROL, 0);
    let c1 = rd(bus, GT_CNT_LO);
    tick_us(bus, 100);
    cmp(rd(bus, GT_CNT_LO), c1)
}

fn gt_comparator_flag(bus: &mut SystemBus) -> Res {
    wr(bus, GT_CONTROL, 0);
    let now = rd(bus, GT_CNT_LO);
    wr(bus, GT_CMP_HI, 0);
    wr(bus, GT_CMP_LO, now.wrapping_add(0x2000));
    wr(bus, GT_CONTROL, 1 | 2);
    let set = wait_until(bus, 5000, |b| rd(b, GT_ISR) & 1 != 0);
    wr(bus, GT_ISR, 1);
    let after = rd(bus, GT_ISR) & 1;
    gt_stop(bus);
    if !set {
        return Err("flag never set".into());
    }
    cmp(after, 0)
}

fn gt_auto_increment(bus: &mut SystemBus) -> Res {
    wr(bus, GT_CONTROL, 0);
    wr(bus, GT_INCR, 0x1000);
    let now = rd(bus, GT_CNT_LO);
    wr(bus, GT_CMP_HI, 0);
    wr(bus, GT_CMP_LO, now.wrapping_add(0x2000));
    wr(bus, GT_CONTROL, 1 | 2 | 8);
    let set = wait_until(bus, 5000, |b| rd(b, GT_ISR) & 1 != 0);
    wr(bus, GT_ISR, 1);
    let next = rd(bus, GT_CMP_LO);
    gt_stop(bus);
    wr(bus, GT_INCR, 0);
    if !set {
        return Err("flag never set".into());
    }
    cmp(next, now.wrapping_add(0x2000).wrapping_add(0x1000))
}

fn gt_irq_gic_ppi27(bus: &mut SystemBus) -> Res {
    gic_enable_ppi(bus, u32::from(GLOBAL_TIMER_PPI));
    let mut taken = [0u32; 2];
    wr(bus, GT_CONTROL, 0);
    let now = rd(bus, GT_CNT_LO);
    wr(bus, GT_CMP_HI, 0);
    wr(bus, GT_CMP_LO, now.wrapping_add(0x2000));
    wr(bus, GT_CONTROL, 1 | 2 | 4);
    for _ in 0..5000 {
        service_irqs(bus, |b, id| timer_handler(b, id, &mut taken));
        if taken[1] >= 1 {
            break;
        }
        tick_us(bus, 10);
    }
    gt_stop(bus);
    if taken[1] >= 1 {
        Ok(())
    } else {
        Err(format!("handled {} times, want >= 1", taken[1]))
    }
}

#[test]
fn conf_scutimer_every_check_passes() {
    let (mut bus, _h) = system();
    println!("conformance: private timer 0xF8F00600, global timer 0xF8F00200");
    let checks: Vec<Check> = vec![
        ("reset_state", reset_state),
        ("pt_load_readback", pt_load_readback),
        (
            "pt_counter_writable_when_stopped",
            pt_counter_writable_when_stopped,
        ),
        ("pt_control_readback", pt_control_readback),
        ("pt_counts_down_and_stops", pt_counts_down_and_stops),
        ("pt_event_flag_w1c", pt_event_flag_w1c),
        ("pt_auto_reload", pt_auto_reload),
        ("pt_one_shot_stops", pt_one_shot_stops),
        ("pt_prescaler_slows", pt_prescaler_slows),
        ("pt_flag_without_irq", pt_flag_without_irq),
        ("pt_irq_gic_ppi29", pt_irq_gic_ppi29),
        ("gt_counter_advances", gt_counter_advances),
        (
            "gt_counter_writable_when_stopped",
            gt_counter_writable_when_stopped,
        ),
        (
            "gt_counter_holds_when_disabled",
            gt_counter_holds_when_disabled,
        ),
        ("gt_comparator_flag", gt_comparator_flag),
        ("gt_auto_increment", gt_auto_increment),
        ("gt_irq_gic_ppi27", gt_irq_gic_ppi27),
    ];
    assert_eq!(checks.len(), 17);
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
    assert!(failures.is_empty(), "failed: {failures:?}");
    println!("RESULT: ALL PASS");
    // The unmapped log must stay empty: every access of the firmware is mapped.
    assert_eq!(bus.unmapped_count(), 0);
}
