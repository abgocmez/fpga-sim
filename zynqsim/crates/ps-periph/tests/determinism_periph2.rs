//! Determinism of the new blocks: the same scripted session, twice, gives the same state hash
//! after every step; a different stimulus gives a different hash.

mod support;

use mem_sys::SystemBus;
use ps_periph::i2c::{I2cPeer, Nack};
use ps_periph::spi::Loopback;
use support::*;

struct Byte(u8);

impl I2cPeer for Byte {
    fn write(&mut self, addr: u8, _d: &[u8]) -> Result<(), Nack> {
        if addr == 0x51 { Ok(()) } else { Err(Nack) }
    }
    fn read(&mut self, addr: u8, len: usize) -> Result<Vec<u8>, Nack> {
        if addr == 0x51 {
            Ok(vec![self.0; len])
        } else {
            Err(Nack)
        }
    }
}

/// Runs the session; `pin` and `tick` vary the stimulus. Returns the hash after every step.
fn session(pin: usize, tick: u64) -> Vec<u64> {
    let (mut bus, h) = system();
    let mut hashes = Vec::new();
    let step = |bus: &SystemBus, hashes: &mut Vec<u64>| hashes.push(bus.state_hash());

    h.gpio.lock().unwrap().set_input(pin, true);
    wr(&mut bus, 0xE000_A000 + 0x204 + 0x18, 1 << 4); // bank 0 INT_TYPE
    step(&bus, &mut hashes);

    h.spi[0].lock().unwrap().set_peer(Box::new(Loopback));
    wr(
        &mut bus,
        0xE000_6000,
        0x0002_0000 | (0b1110 << 10) | (1 << 0) | (1 << 16),
    );
    wr(&mut bus, 0xE000_6000 + 0x14, 1);
    for b in 0..8u32 {
        wr(&mut bus, 0xE000_6000 + 0x1C, b * 3);
    }
    let cr = rd(&mut bus, 0xE000_6000);
    wr(&mut bus, 0xE000_6000, cr | (1 << 15));
    step(&bus, &mut hashes);

    h.i2c[0].lock().unwrap().set_peer(Box::new(Byte(0x42)));
    wr(&mut bus, 0xE000_4000, 0b10 | 1);
    wr(&mut bus, 0xE000_4008, 0x51);
    wr(&mut bus, 0xE000_4014, 4);
    step(&bus, &mut hashes);

    wr(&mut bus, 0xF800_7000 + 0x100 + 0x10, 1 << 26);
    step(&bus, &mut hashes);

    wr(&mut bus, 0xE000_8000, 0);
    wr(&mut bus, 0xE000_8000 + 0x04, 2);
    wr(&mut bus, 0xE000_8000, 2);
    for k in 0..4u64 {
        wr(&mut bus, 0xE000_8000 + 0x30 + 4 * k, k as u32 + 1);
    }
    step(&bus, &mut hashes);

    bus.tick(tick);
    step(&bus, &mut hashes);
    hashes
}

#[test]
fn the_same_session_gives_the_same_hash_at_every_step() {
    let a = session(20, 12_345);
    let b = session(20, 12_345);
    assert_eq!(a, b);
    assert_eq!(a.len(), 6);
}

#[test]
fn a_different_stimulus_gives_a_different_hash() {
    let a = session(20, 12_345);
    let b = session(21, 12_345);
    assert_ne!(a.last(), b.last());
}
