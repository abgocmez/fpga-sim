//! Integration: the private timer drives the GIC through a software-visible sequence. Enable
//! PPI 29 in the distributor, program the timer, tick the system, observe the core's IRQ
//! line, acknowledge through ICCIAR, end the interrupt through ICCEOIR.

mod support;

use mem_sys::Bus;
use support::*;

const PT_LOAD: u64 = 0xF8F0_0600;
const PT_COUNTER: u64 = 0xF8F0_0604;
const PT_CONTROL: u64 = 0xF8F0_0608;
const PT_ISR: u64 = 0xF8F0_060C;
const ICCIAR: u64 = 0xF8F0_010C;
const ICCEOIR: u64 = 0xF8F0_0110;
const ICDICER0: u64 = 0xF8F0_1180;

/// One period of the timer in PERIPHCLK ticks, which is two CPU cycles per tick.
const LOAD: u32 = 1000;

#[test]
fn private_timer_interrupt_is_acknowledged_and_ended_through_the_gic() {
    let (mut bus, _h) = system();
    gic_up(&mut bus);
    gic_enable_ppi(&mut bus, 29);

    // Nothing is pending before the timer runs.
    assert!(!bus.core_irq(0));

    wr(&mut bus, PT_LOAD, LOAD);
    wr(&mut bus, PT_COUNTER, LOAD);
    // Enable, auto-reload, IRQ enable.
    wr(&mut bus, PT_CONTROL, 0b111);

    // 999 PERIPHCLK ticks (1998 CPU cycles): the counter is at 1, no interrupt yet.
    bus.tick(1998);
    assert!(!bus.core_irq(0));
    assert_eq!(rd(&mut bus, PT_COUNTER), 1);

    // The next PERIPHCLK tick reaches zero: the flag sets and PPI 29 asserts on core 0.
    bus.tick(2);
    assert!(bus.core_irq(0), "PPI 29 reaches core 0");
    assert!(!bus.core_irq(1), "the timer and its PPI belong to core 0");
    assert_eq!(rd(&mut bus, PT_ISR) & 1, 1);

    // Acknowledge: ICCIAR returns the ID, and the interrupt is now active.
    let iar = Bus::read(&mut bus.core(0), ICCIAR, 4).unwrap();
    assert_eq!(iar, 29);
    assert!(
        !bus.core_irq(0),
        "an active interrupt is not signalled again"
    );

    // The handler clears the timer's flag, then ends the interrupt.
    wr(&mut bus, PT_ISR, 1);
    Bus::write(&mut bus.core(0), ICCEOIR, 4, iar).unwrap();
    assert!(!bus.core_irq(0), "source cleared and interrupt ended");

    // Auto-reload: the next event comes LOAD + 1 = 1001 PERIPHCLK ticks later.
    bus.tick(2000);
    assert!(!bus.core_irq(0));
    bus.tick(2);
    assert!(bus.core_irq(0), "second event after one period");
    let iar = Bus::read(&mut bus.core(0), ICCIAR, 4).unwrap();
    assert_eq!(iar, 29);
    wr(&mut bus, PT_ISR, 1);
    Bus::write(&mut bus.core(0), ICCEOIR, 4, iar).unwrap();

    // Disabling PPI 29 in the distributor silences the next event.
    wr(&mut bus, PT_CONTROL, 0);
    wr(&mut bus, ICDICER0, 1 << 29);
    wr(&mut bus, PT_CONTROL, 0b111);
    bus.tick(4000);
    assert!(!bus.core_irq(0), "disabled interrupt is not signalled");
    assert_eq!(
        rd(&mut bus, PT_ISR) & 1,
        1,
        "but the timer still records the event"
    );
}

#[test]
fn the_priority_mask_holds_back_an_enabled_timer_interrupt() {
    let (mut bus, _h) = system();
    gic_up(&mut bus);
    gic_enable_ppi(&mut bus, 29);
    wr(&mut bus, 0xF8F0_0104, 0); // ICCPMR: mask everything
    wr(&mut bus, PT_LOAD, 10);
    wr(&mut bus, PT_COUNTER, 10);
    wr(&mut bus, PT_CONTROL, 0b111);
    tick_us(&mut bus, 1);
    assert!(!bus.core_irq(0), "priority 0xA0 is not below the mask 0");
    wr(&mut bus, 0xF8F0_0104, 0xF0);
    assert!(bus.core_irq(0));
}

#[test]
fn the_interrupt_is_taken_by_the_service_loop_and_the_timer_flag_is_cleared() {
    let (mut bus, _h) = system();
    gic_up(&mut bus);
    gic_enable_ppi(&mut bus, 29);
    wr(&mut bus, PT_LOAD, LOAD);
    wr(&mut bus, PT_COUNTER, LOAD);
    wr(&mut bus, PT_CONTROL, 0b111);
    // 4 us = 1333 PERIPHCLK ticks: the first event at tick 1000, the reload, the next event at
    // tick 2001 is still in the future. The service loop takes the one pending interrupt.
    tick_us(&mut bus, 4);
    let taken = service_irqs(&mut bus, |b, id| {
        assert_eq!(id, 29);
        wr(b, PT_ISR, 1);
    });
    assert_eq!(taken, vec![29]);
    assert!(!bus.core_irq(0));
}
