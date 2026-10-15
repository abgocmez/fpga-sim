//! I2C slave decoding from the SCL and SDA pins of a Pmod port.
//!
//! [`I2cSlave`] is the pin-level state machine: it watches the levels the
//! *master* drives on SCL and SDA, finds START, repeated START and STOP, shifts
//! the 7-bit address and the data bytes, and says whether the slave pulls SDA
//! low (ACK, or a 0 data bit). The bytes go to an [`I2cDevice`]. For devices
//! built around a register pointer (nearly every sensor) [`PointerSlave`]
//! implements the byte protocol over a [`Registers`] file; the OV5640's SCCB
//! uses it with 16-bit register addresses.
//!
//! Not modelled: clock stretching, 10-bit addresses, general call, a second
//! master (arbitration), bus speed limits. The bus is open drain: a level of 1
//! means released, and the slave's own drive is combined with the master's by
//! the PL side (`pin_in = master & slave`).

/// A device on an I2C bus, seen a byte at a time.
pub trait I2cDevice {
    /// The 7-bit address the device answers to.
    fn address(&self) -> u8;

    /// The device was addressed (and ACKed) for a write (`read` false) or a
    /// read. Called again after a repeated START.
    fn start(&mut self, _read: bool) {}

    /// The master wrote `byte`. Returns true to ACK it.
    fn write(&mut self, byte: u8) -> bool;

    /// The master reads: the next byte to send.
    fn read(&mut self) -> u8;

    /// STOP, after a transfer that addressed this device.
    fn stop(&mut self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// No transfer, or one for another address.
    Idle,
    /// Shifting in the address byte.
    Addr,
    /// The address byte matched; the ACK clock is running.
    AddrAck { read: bool },
    /// Shifting in a data byte from the master.
    Write,
    /// ACK clock of a written byte.
    WriteAck,
    /// Shifting a byte out to the master.
    Read,
    /// The master's ACK/NACK clock after a read byte.
    ReadAck,
    /// Not addressed, or NACKed: wait for STOP or START.
    Ignore,
}

/// The pin-level I2C slave state machine.
#[derive(Clone, Debug)]
pub struct I2cSlave {
    scl_bit: u8,
    sda_bit: u8,
    scl: bool,
    sda: bool,
    sda_out: bool,
    state: State,
    shift: u8,
    bits: u8,
    out: u8,
    master_ack: bool,
    addressed: bool,
    starts: u32,
    stops: u32,
}

impl I2cSlave {
    /// A slave on pins `scl_bit` and `sda_bit` of the port (bit numbers of the
    /// eight-pin vector).
    pub fn new(scl_bit: u8, sda_bit: u8) -> I2cSlave {
        I2cSlave {
            scl_bit,
            sda_bit,
            scl: true,
            sda: true,
            sda_out: true,
            state: State::Idle,
            shift: 0,
            bits: 0,
            out: 0,
            master_ack: false,
            addressed: false,
            starts: 0,
            stops: 0,
        }
    }

    /// True while the slave leaves SDA released (high); false when it pulls
    /// SDA low.
    pub fn sda_released(&self) -> bool {
        self.sda_out
    }

    /// The `(starts, stops)` seen, repeated STARTs included.
    pub fn conditions(&self) -> (u32, u32) {
        (self.starts, self.stops)
    }

    /// The master-driven levels of the port changed to `levels`.
    pub fn pins<D: I2cDevice + ?Sized>(&mut self, dev: &mut D, levels: u8) {
        let scl = levels >> self.scl_bit & 1 != 0;
        let sda = levels >> self.sda_bit & 1 != 0;
        let (pscl, psda) = (self.scl, self.sda);
        self.scl = scl;
        self.sda = sda;
        if scl && pscl {
            // SCL stayed high: an SDA edge is a START or a STOP.
            if psda && !sda {
                self.start_condition();
            } else if !psda && sda {
                self.stop_condition(dev);
            }
        } else if scl && !pscl {
            self.rising(sda);
        } else if !scl && pscl {
            self.falling(dev);
        }
    }

    fn start_condition(&mut self) {
        self.starts += 1;
        self.state = State::Addr;
        self.bits = 0;
        self.shift = 0;
        self.sda_out = true;
    }

    fn stop_condition<D: I2cDevice + ?Sized>(&mut self, dev: &mut D) {
        self.stops += 1;
        if self.addressed {
            dev.stop();
        }
        self.addressed = false;
        self.state = State::Idle;
        self.sda_out = true;
    }

    /// SCL rose: the master's SDA is valid; sample it.
    fn rising(&mut self, sda: bool) {
        match self.state {
            State::Addr | State::Write => {
                self.shift = self.shift << 1 | u8::from(sda);
                self.bits = self.bits.saturating_add(1);
            }
            State::Read => self.bits = self.bits.saturating_add(1),
            State::ReadAck => self.master_ack = !sda,
            _ => {}
        }
    }

    /// SCL fell: the slave may change SDA now.
    fn falling<D: I2cDevice + ?Sized>(&mut self, dev: &mut D) {
        match self.state {
            State::Addr if self.bits == 8 => {
                let addr = self.shift >> 1;
                let read = self.shift & 1 != 0;
                if addr == dev.address() {
                    self.addressed = true;
                    dev.start(read);
                    self.state = State::AddrAck { read };
                    self.sda_out = false; // ACK
                } else {
                    self.state = State::Ignore;
                }
            }
            State::AddrAck { read } => {
                self.bits = 0;
                self.shift = 0;
                if read {
                    self.load(dev);
                } else {
                    self.state = State::Write;
                    self.sda_out = true;
                }
            }
            State::Write if self.bits == 8 => {
                if dev.write(self.shift) {
                    self.state = State::WriteAck;
                    self.sda_out = false;
                } else {
                    self.state = State::Ignore;
                    self.sda_out = true;
                }
            }
            State::WriteAck => {
                self.bits = 0;
                self.shift = 0;
                self.state = State::Write;
                self.sda_out = true;
            }
            State::Read => {
                if self.bits >= 8 {
                    self.state = State::ReadAck;
                    self.sda_out = true;
                } else {
                    self.sda_out = self.out >> (7 - self.bits) & 1 != 0;
                }
            }
            State::ReadAck => {
                if self.master_ack {
                    self.bits = 0;
                    self.load(dev);
                } else {
                    self.state = State::Ignore;
                    self.sda_out = true;
                }
            }
            _ => {}
        }
    }

    /// Starts sending the next byte: its MSB goes on SDA now.
    fn load<D: I2cDevice + ?Sized>(&mut self, dev: &mut D) {
        self.out = dev.read();
        self.state = State::Read;
        self.sda_out = self.out & 0x80 != 0;
    }
}

/// A register file behind a pointer-addressed I2C device.
pub trait Registers {
    /// Reads register `reg`. May have side effects (latching, clear on read).
    fn read(&mut self, reg: u32) -> u8;

    /// Writes register `reg`.
    fn write(&mut self, reg: u32, value: u8);
}

/// The common sensor protocol: after the address (write), the first one or
/// two bytes set a register pointer, further written bytes go to consecutive
/// registers, and reads return consecutive registers from the pointer. The
/// pointer survives STOP, so "write pointer, STOP, read" works as well as a
/// repeated START.
#[derive(Clone, Debug)]
pub struct PointerSlave<R: Registers> {
    /// The register file.
    pub regs: R,
    addr: u8,
    addr_bytes: u8,
    ptr: u32,
    got: u8,
}

impl<R: Registers> PointerSlave<R> {
    /// A device at 7-bit `addr` whose register pointer is `addr_bytes` (1 or
    /// 2) bytes long, most significant first.
    pub fn new(addr: u8, addr_bytes: u8, regs: R) -> PointerSlave<R> {
        PointerSlave {
            regs,
            addr,
            addr_bytes: addr_bytes.clamp(1, 2),
            ptr: 0,
            got: 0,
        }
    }

    /// The current register pointer.
    pub fn pointer(&self) -> u32 {
        self.ptr
    }
}

impl<R: Registers> I2cDevice for PointerSlave<R> {
    fn address(&self) -> u8 {
        self.addr
    }

    fn start(&mut self, read: bool) {
        if !read {
            self.got = 0;
        }
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.got < self.addr_bytes {
            self.ptr = if self.got == 0 {
                u32::from(byte)
            } else {
                (self.ptr << 8 | u32::from(byte)) & 0xFFFF
            };
            self.got += 1;
        } else {
            self.regs.write(self.ptr, byte);
            self.ptr = self.ptr.wrapping_add(1);
        }
        true
    }

    fn read(&mut self) -> u8 {
        let v = self.regs.read(self.ptr);
        self.ptr = self.ptr.wrapping_add(1);
        v
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SCL: u8 = 1 << 2;
    pub const SDA: u8 = 1 << 3;

    /// A bit-banging master that drives an [`I2cSlave`] and combines its SDA
    /// with the slave's, like the wired AND on the bus.
    pub struct Master<D: I2cDevice> {
        pub slave: I2cSlave,
        pub dev: D,
        scl: bool,
        sda: bool,
        pub t: u64,
    }

    impl<D: I2cDevice> Master<D> {
        pub fn new(dev: D) -> Master<D> {
            Master {
                slave: I2cSlave::new(2, 3),
                dev,
                scl: true,
                sda: true,
                t: 0,
            }
        }

        fn drive(&mut self, scl: bool, sda: bool) {
            self.scl = scl;
            self.sda = sda;
            self.t += 1;
            let levels = if scl { SCL } else { 0 } | if sda { SDA } else { 0 };
            self.slave.pins(&mut self.dev, levels);
        }

        /// SDA as the master sees it: its own drive wired-ANDed with the slave's.
        fn bus_sda(&self) -> bool {
            self.sda && self.slave.sda_released()
        }

        pub fn start(&mut self) {
            self.drive(true, true);
            self.drive(true, false);
            self.drive(false, false);
        }

        pub fn stop(&mut self) {
            self.drive(false, false);
            self.drive(true, false);
            self.drive(true, true);
        }

        /// Writes a byte and returns whether the slave ACKed it.
        pub fn write_byte(&mut self, b: u8) -> bool {
            for i in (0..8).rev() {
                let bit = b >> i & 1 != 0;
                self.drive(false, bit);
                self.drive(true, bit);
                self.drive(false, bit);
            }
            self.drive(false, true); // release for the ACK
            self.drive(true, true);
            let ack = !self.bus_sda();
            self.drive(false, true);
            ack
        }

        /// Reads a byte, answering with ACK (`ack`) or NACK.
        pub fn read_byte(&mut self, ack: bool) -> u8 {
            let mut v = 0u8;
            self.drive(false, true);
            for _ in 0..8 {
                self.drive(true, true);
                v = v << 1 | u8::from(self.bus_sda());
                self.drive(false, true);
            }
            let a = !ack; // ACK pulls SDA low
            self.drive(false, a);
            self.drive(true, a);
            self.drive(false, a);
            self.drive(false, true);
            v
        }

        /// START, address + W, `bytes`, STOP; the ACK of every byte.
        pub fn write(&mut self, addr: u8, bytes: &[u8]) -> Vec<bool> {
            self.start();
            let mut acks = vec![self.write_byte(addr << 1)];
            for &b in bytes {
                acks.push(self.write_byte(b));
            }
            self.stop();
            acks
        }
    }

    /// A tiny device: 16 registers, pointer protocol.
    struct Mem(Vec<u8>);

    impl Registers for Mem {
        fn read(&mut self, reg: u32) -> u8 {
            self.0.get(reg as usize).copied().unwrap_or(0xFF)
        }
        fn write(&mut self, reg: u32, v: u8) {
            if let Some(r) = self.0.get_mut(reg as usize) {
                *r = v;
            }
        }
    }

    fn dev() -> PointerSlave<Mem> {
        let mut m = vec![0u8; 16];
        for (i, v) in m.iter_mut().enumerate() {
            *v = 0xA0 + i as u8;
        }
        PointerSlave::new(0x4B, 1, Mem(m))
    }

    #[test]
    fn writes_set_the_pointer_then_registers() {
        let mut m = Master::new(dev());
        let acks = m.write(0x4B, &[0x03, 0x11, 0x22]);
        assert_eq!(acks, vec![true, true, true, true]);
        assert_eq!(m.dev.regs.0[3], 0x11);
        assert_eq!(m.dev.regs.0[4], 0x22, "the pointer auto-increments");
        assert_eq!(m.slave.conditions(), (1, 1));
    }

    #[test]
    fn a_read_after_repeated_start_returns_consecutive_registers() {
        let mut m = Master::new(dev());
        m.start();
        assert!(m.write_byte(0x4B << 1));
        assert!(m.write_byte(0x02));
        m.start(); // repeated START: SDA falls while SCL is high
        assert!(m.write_byte(0x4B << 1 | 1));
        assert_eq!(m.read_byte(true), 0xA2);
        assert_eq!(m.read_byte(true), 0xA3);
        assert_eq!(m.read_byte(false), 0xA4, "NACK ends the read");
        m.stop();
        assert_eq!(m.slave.conditions().1, 1);
        assert!(m.slave.sda_released());
    }

    #[test]
    fn the_pointer_survives_a_stop() {
        let mut m = Master::new(dev());
        m.write(0x4B, &[0x07]);
        m.start();
        assert!(m.write_byte(0x4B << 1 | 1));
        assert_eq!(m.read_byte(false), 0xA7);
        m.stop();
    }

    #[test]
    fn another_address_is_not_acknowledged_and_ignored() {
        let mut m = Master::new(dev());
        let acks = m.write(0x48, &[0x03, 0x99]);
        assert!(!acks[0], "address NACK");
        assert_eq!(m.dev.regs.0[3], 0xA3, "nothing written");
        // Still works afterwards.
        assert!(m.write(0x4B, &[0x01, 0x5A])[0]);
        assert_eq!(m.dev.regs.0[1], 0x5A);
    }

    #[test]
    fn a_device_nack_ends_the_write() {
        struct Picky(Vec<u8>);
        impl I2cDevice for Picky {
            fn address(&self) -> u8 {
                0x10
            }
            fn write(&mut self, b: u8) -> bool {
                self.0.push(b);
                b != 0xFF
            }
            fn read(&mut self) -> u8 {
                0
            }
        }
        let mut m = Master::new(Picky(Vec::new()));
        let acks = m.write(0x10, &[1, 0xFF, 3]);
        assert_eq!(acks, vec![true, true, false, false]);
        assert_eq!(m.dev.0, vec![1, 0xFF], "bytes after the NACK are ignored");
    }

    #[test]
    fn two_byte_pointers_for_sccb_style_devices() {
        let mut d = PointerSlave::new(0x3C, 2, Mem(vec![0u8; 0x10000]));
        d.regs.0[0x300A] = 0x56;
        d.regs.0[0x300B] = 0x40;
        let mut m = Master::new(d);
        m.write(0x3C, &[0x30, 0x0A]);
        m.start();
        assert!(m.write_byte(0x3C << 1 | 1));
        assert_eq!(m.read_byte(true), 0x56);
        assert_eq!(m.read_byte(false), 0x40);
        m.stop();
        m.write(0x3C, &[0x50, 0x3D, 0xA5]);
        assert_eq!(m.dev.regs.0[0x503D], 0xA5);
        assert_eq!(m.dev.pointer(), 0x503E);
    }
}
