//! Determinism: the same scripted session, with the same ticks, gives the same state hash at
//! every step, and RAM snapshots round-trip. The session touches every block: SLCR, both
//! UARTs (TX and RX), TTC0, the GIC with the private timer and the TTC interrupt, DDR and OCM,
//! and an unmapped access under the RAZ/WI policy.

mod support;

use mem_sys::{Bus, SystemBus, UnmappedPolicy};
use ps_periph::{PsConfig, zynq_system};
use support::*;

/// Runs the session with the given tick sizes (CPU cycles) and returns the state hash after
/// every step, plus the final bus.
fn session(ticks: &[u64]) -> (Vec<u64>, SystemBus) {
    let cfg = PsConfig {
        policy: UnmappedPolicy::RazWi,
        ..PsConfig::default()
    };
    let (mut bus, h) = zynq_system(&cfg).unwrap();
    let mut hashes = Vec::new();
    let step = |bus: &SystemBus, hashes: &mut Vec<u64>| hashes.push(bus.state_hash());

    // SLCR: unlock, set a clock register.
    wr(&mut bus, 0xF800_0008, 0xDF0D);
    wr(&mut bus, 0xF800_0120, 0x1234_5678);
    step(&bus, &mut hashes);

    // UART0 at 9600-ish baud: TX and RX on, divisor 651 x 16, five bytes out.
    wr(&mut bus, 0xE000_0000, 0x14);
    wr(&mut bus, 0xE000_0018, 651);
    wr(&mut bus, 0xE000_0034, 15);
    for b in b"hello" {
        Bus::write(&mut bus, 0xE000_0030, 1, u64::from(*b)).unwrap();
    }
    step(&bus, &mut hashes);

    // UART1: RX on, bytes injected by the host.
    wr(&mut bus, 0xE000_1000, 0x14);
    assert_eq!(h.uart1.borrow_mut().inject_rx(b"xy"), 2);
    step(&bus, &mut hashes);

    // DDR and OCM writes with a pattern.
    for i in 0..64u64 {
        Bus::write(
            &mut bus,
            0x0010_0000 + 4 * i,
            4,
            i.wrapping_mul(0x9E37_79B9),
        )
        .unwrap();
        Bus::write(&mut bus, 0xFFFC_0000 + 4 * i, 4, i ^ 0x55).unwrap();
    }
    // An unmapped read: RAZ, logged.
    assert_eq!(Bus::read(&mut bus, 0x3000_0000, 4), Ok(0));
    step(&bus, &mut hashes);

    // TTC0 interval interrupt through the GIC, and the private timer with PPI 29.
    gic_up(&mut bus);
    gic_enable_spi(&mut bus, 42);
    gic_enable_ppi(&mut bus, 29);
    wr(&mut bus, 0xF800_1024, 500);
    wr(&mut bus, 0xF800_1060, 1);
    wr(&mut bus, 0xF800_100C, 2);
    wr(&mut bus, 0xF8F0_0600, 300);
    wr(&mut bus, 0xF8F0_0604, 300);
    wr(&mut bus, 0xF8F0_0608, 0b111);
    step(&bus, &mut hashes);

    for &t in ticks {
        bus.tick(t);
        // The core takes whatever is pending, as the firmware would.
        let _ = service_irqs(&mut bus, |b, id| {
            if id == 29 {
                wr(b, 0xF8F0_060C, 1);
            } else if id == 42 {
                wr(b, 0xF800_1054, 1);
            }
        });
        step(&bus, &mut hashes);
    }
    (hashes, bus)
}

const TICKS_A: &[u64] = &[1, 997, 100_000, 3, 250_000, 1_000, 7_777];

#[test]
fn the_same_session_gives_the_same_hash_at_every_step() {
    let (first, _) = session(TICKS_A);
    let (second, _) = session(TICKS_A);
    assert_eq!(first, second);
    // The hashes move: a constant hash would mean the state is not hashed.
    let mut distinct = first.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() > first.len() / 2,
        "only {} distinct hashes",
        distinct.len()
    );
}

#[test]
fn five_runs_in_one_process_agree() {
    let runs: Vec<Vec<u64>> = (0..5).map(|_| session(TICKS_A).0).collect();
    assert!(runs.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn a_different_tick_plan_changes_the_final_hash() {
    let (a, _) = session(TICKS_A);
    let mut longer = TICKS_A.to_vec();
    longer.push(1);
    let (b, _) = session(&longer);
    assert_ne!(a.last(), b.last());
}

#[test]
fn ram_snapshot_round_trips_after_a_session() {
    let (_, bus) = session(TICKS_A);
    let snap = bus.ram_snapshot();
    let cfg = PsConfig {
        policy: UnmappedPolicy::RazWi,
        ..PsConfig::default()
    };
    let (mut fresh, _) = zynq_system(&cfg).unwrap();
    fresh.ram_restore(&snap).unwrap();
    assert_eq!(fresh.ram_snapshot(), snap);
    assert_eq!(Bus::read(&mut fresh, 0x0010_0004, 4), Ok(0x9E37_79B9));
}

#[test]
fn unmapped_accesses_are_counted_in_the_hash() {
    let (_, bus) = session(&[1]);
    assert_eq!(bus.unmapped_count(), 1);
    assert_eq!(bus.unmapped_log()[0].addr, 0x3000_0000);
}
