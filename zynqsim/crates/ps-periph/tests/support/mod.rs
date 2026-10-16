//! Helpers shared by the integration tests: a PS bus, 32-bit accesses, time in microseconds
//! and a GIC service loop that plays the interrupt handler of the core.

#![allow(dead_code)]

use mem_sys::{Bus, SystemBus};
use ps_periph::{PsConfig, PsHandles, zynq_system};

/// CPU cycles per microsecond (666.67 MHz).
pub fn cycles_for_us(us: u64) -> u64 {
    us * 2000 / 3
}

/// The PS bus with default configuration.
pub fn system() -> (SystemBus, PsHandles) {
    zynq_system(&PsConfig::default()).expect("default PS builds")
}

/// A 32-bit read through the core-0 view.
pub fn rd(bus: &mut SystemBus, addr: u64) -> u32 {
    Bus::read(bus, addr, 4).expect("mapped read") as u32
}

/// A 32-bit write through the core-0 view.
pub fn wr(bus: &mut SystemBus, addr: u64, value: u32) {
    Bus::write(bus, addr, 4, u64::from(value)).expect("mapped write");
}

/// Advances the whole system by `us` microseconds.
pub fn tick_us(bus: &mut SystemBus, us: u64) {
    bus.tick(cycles_for_us(us));
}

/// Polls `cond` every 10 us, at most `max` times, as the firmware's `wait_until` does.
pub fn wait_until(
    bus: &mut SystemBus,
    max: u32,
    mut cond: impl FnMut(&mut SystemBus) -> bool,
) -> bool {
    for _ in 0..max {
        if cond(bus) {
            return true;
        }
        tick_us(bus, 10);
    }
    cond(bus)
}

/// Enables the distributor and core 0's CPU interface, with the priority mask open.
pub fn gic_up(bus: &mut SystemBus) {
    wr(bus, 0xF8F0_1000, 1); // ICDDCR
    wr(bus, 0xF8F0_0104, 0xF0); // ICCPMR
    wr(bus, 0xF8F0_0100, 1); // ICCICR
}

/// Enables a private interrupt (PPI 16..=31) for core 0 at priority 0xA0.
pub fn gic_enable_ppi(bus: &mut SystemBus, id: u32) {
    wr(bus, 0xF8F0_1100, 1 << id); // ICDISER word 0
    Bus::write(bus, 0xF8F0_1400 + u64::from(id), 1, 0xA0).expect("priority");
}

/// Enables a shared interrupt (SPI 32..=95) for core 0 at priority 0xA0, level-sensitive.
pub fn gic_enable_spi(bus: &mut SystemBus, id: u32) {
    let word = 0xF8F0_1100 + u64::from(id / 32) * 4;
    wr(bus, word, 1 << (id % 32));
    Bus::write(bus, 0xF8F0_1400 + u64::from(id), 1, 0xA0).expect("priority");
    Bus::write(bus, 0xF8F0_1800 + u64::from(id), 1, 0x01).expect("target core 0");
}

/// Plays the core's interrupt handler: while the core's IRQ line is asserted, acknowledges
/// the interrupt (ICCIAR), runs `handler` for its ID, and ends it (ICCEOIR). Returns the IDs
/// taken, in order. Stops after 32 interrupts so that a handler that does not clear its
/// source cannot hang a test.
pub fn service_irqs(bus: &mut SystemBus, mut handler: impl FnMut(&mut SystemBus, u32)) -> Vec<u32> {
    let mut taken = Vec::new();
    while bus.core_irq(0) && taken.len() < 32 {
        let iar = Bus::read(&mut bus.core(0), 0xF8F0_010C, 4).expect("ICCIAR") as u32;
        let id = iar & 0x3FF;
        if id == 1023 {
            break;
        }
        handler(bus, id);
        Bus::write(&mut bus.core(0), 0xF8F0_0110, 4, u64::from(iar)).expect("ICCEOIR");
        taken.push(id);
    }
    taken
}
