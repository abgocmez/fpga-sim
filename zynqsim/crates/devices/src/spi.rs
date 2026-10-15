//! SPI slave decoding from the CS, SCLK, MOSI and MISO pins of a Pmod port.
//!
//! [`SpiSlave`] watches the pins the master drives (CS active low), shifts bytes
//! MSB first in any of the four SPI modes and drives MISO. The bytes go to an
//! [`SpiDevice`]. Because the next byte a slave shifts out is chosen while the
//! current one arrives, the device interface is "byte in, next byte out": an
//! [`SpiDevice::exchange`] call returns what will be shifted out during the
//! byte that follows the one just received.
//!
//! Not modelled: 3-wire (shared data pin) operation, LSB-first, words that are
//! not 8 bits (a 16-bit frame is two bytes), setup and hold times.

use crate::PinDrive;

/// A device on an SPI bus, seen a byte at a time.
pub trait SpiDevice {
    /// CS fell. Returns the first byte to shift out.
    fn select(&mut self) -> u8 {
        0
    }

    /// A whole byte arrived from the master. Returns the byte to shift out
    /// during the *next* byte.
    fn exchange(&mut self, mosi: u8) -> u8;

    /// CS rose (also after a byte cut short).
    fn deselect(&mut self) {}
}

/// The pin-level SPI slave.
#[derive(Clone, Debug)]
pub struct SpiSlave {
    cs_bit: u8,
    sclk_bit: u8,
    mosi_bit: u8,
    miso_bit: u8,
    cpol: bool,
    cpha: bool,
    selected: bool,
    sclk: bool,
    in_shift: u8,
    in_bits: u8,
    out: u8,
    out_idx: u8,
    pending: u8,
    miso: bool,
    bytes: u32,
}

impl SpiSlave {
    /// A slave with the given pin bits and SPI `mode` 0..=3 (CPOL = bit 1,
    /// CPHA = bit 0).
    pub fn new(cs_bit: u8, mosi_bit: u8, miso_bit: u8, sclk_bit: u8, mode: u8) -> SpiSlave {
        let cpol = mode & 2 != 0;
        SpiSlave {
            cs_bit,
            sclk_bit,
            mosi_bit,
            miso_bit,
            cpol,
            cpha: mode & 1 != 0,
            selected: false,
            sclk: cpol,
            in_shift: 0,
            in_bits: 0,
            out: 0,
            out_idx: 0,
            pending: 0,
            miso: false,
            bytes: 0,
        }
    }

    /// True while CS is low.
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Complete bytes received since the start.
    pub fn bytes(&self) -> u32 {
        self.bytes
    }

    /// The pin the slave drives: MISO while selected, released otherwise.
    pub fn drive(&self) -> PinDrive {
        if self.selected {
            PinDrive {
                mask: 1 << self.miso_bit,
                levels: u8::from(self.miso) << self.miso_bit,
            }
        } else {
            PinDrive::NONE
        }
    }

    /// The master-driven levels of the port changed to `levels`.
    pub fn pins<D: SpiDevice + ?Sized>(&mut self, dev: &mut D, levels: u8) {
        let selected = levels >> self.cs_bit & 1 == 0;
        let sclk = levels >> self.sclk_bit & 1 != 0;
        let mosi = levels >> self.mosi_bit & 1 != 0;
        if !selected {
            if self.selected {
                dev.deselect();
            }
            self.selected = false;
            self.sclk = sclk;
            return;
        }
        if !self.selected {
            self.selected = true;
            self.sclk = sclk;
            self.in_shift = 0;
            self.in_bits = 0;
            self.out = dev.select();
            self.pending = self.out;
            self.miso = self.out & 0x80 != 0;
            // Mode 0/2: the first bit is out before the first clock edge.
            self.out_idx = if self.cpha { 0 } else { 1 };
            return;
        }
        if sclk == self.sclk {
            return;
        }
        self.sclk = sclk;
        // Leading edge: the clock leaves its idle level.
        let leading = sclk != self.cpol;
        if leading != self.cpha {
            // Sampling edge.
            self.in_shift = self.in_shift << 1 | u8::from(mosi);
            self.in_bits += 1;
            if self.in_bits == 8 {
                self.pending = dev.exchange(self.in_shift);
                self.in_bits = 0;
                self.in_shift = 0;
                self.bytes += 1;
            }
        } else {
            // Shifting edge.
            if self.out_idx >= 8 {
                self.out = self.pending;
                self.out_idx = 0;
            }
            self.miso = self.out >> (7 - self.out_idx) & 1 != 0;
            self.out_idx += 1;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const CS: u8 = 1 << 0;
    pub const MOSI: u8 = 1 << 1;
    pub const MISO: u8 = 1 << 2;
    pub const SCLK: u8 = 1 << 3;

    /// A master that clocks bytes through a slave in a given mode and returns
    /// what it samples on MISO.
    pub struct Master<D: SpiDevice> {
        pub slave: SpiSlave,
        pub dev: D,
        mode: u8,
        cs: bool,
        sclk: bool,
        mosi: bool,
    }

    impl<D: SpiDevice> Master<D> {
        pub fn new(dev: D, mode: u8) -> Master<D> {
            let sclk = mode & 2 != 0;
            let mut m = Master {
                slave: SpiSlave::new(0, 1, 2, 3, mode),
                dev,
                mode,
                cs: true,
                sclk,
                mosi: false,
            };
            m.push();
            m
        }

        fn push(&mut self) {
            let lv = (u8::from(self.cs) * CS)
                | (u8::from(self.mosi) * MOSI)
                | (u8::from(self.sclk) * SCLK);
            self.slave.pins(&mut self.dev, lv);
        }

        fn miso(&self) -> bool {
            let d = self.slave.drive();
            // Undriven reads as 1 (pull-up).
            d.mask & MISO == 0 || d.levels & MISO != 0
        }

        pub fn select(&mut self) {
            self.cs = false;
            self.push();
        }

        pub fn deselect(&mut self) {
            self.cs = true;
            self.push();
        }

        /// Clocks one byte out of the master and returns the byte sampled.
        pub fn byte(&mut self, tx: u8) -> u8 {
            let cpol = self.mode & 2 != 0;
            let cpha = self.mode & 1 != 0;
            let mut rx = 0u8;
            for i in (0..8).rev() {
                let bit = tx >> i & 1 != 0;
                if !cpha {
                    // Data valid before the leading edge; sample on it.
                    self.mosi = bit;
                    self.push();
                    self.sclk = !cpol;
                    self.push();
                    rx = rx << 1 | u8::from(self.miso());
                    self.sclk = cpol;
                    self.push();
                } else {
                    // Data changes on the leading edge; sample on the trailing.
                    self.sclk = !cpol;
                    self.push();
                    self.mosi = bit;
                    self.push();
                    self.sclk = cpol;
                    self.push();
                    rx = rx << 1 | u8::from(self.miso());
                }
            }
            rx
        }
    }

    /// Replies with 0xA5 first and then the complement of what it received.
    struct Complement {
        seen: Vec<u8>,
        deselects: u32,
    }

    impl SpiDevice for Complement {
        fn select(&mut self) -> u8 {
            0xA5
        }
        fn exchange(&mut self, mosi: u8) -> u8 {
            self.seen.push(mosi);
            !mosi
        }
        fn deselect(&mut self) {
            self.deselects += 1;
        }
    }

    fn dev() -> Complement {
        Complement {
            seen: Vec::new(),
            deselects: 0,
        }
    }

    #[test]
    fn all_four_modes_exchange_bytes_msb_first() {
        for mode in 0..4u8 {
            let mut m = Master::new(dev(), mode);
            m.select();
            let rx: Vec<u8> = [0x12, 0xC3, 0x00, 0xFF]
                .iter()
                .map(|&b| m.byte(b))
                .collect();
            m.deselect();
            assert_eq!(rx, vec![0xA5, !0x12, !0xC3, !0x00], "mode {mode}");
            assert_eq!(m.dev.seen, vec![0x12, 0xC3, 0x00, 0xFF], "mode {mode}");
            assert_eq!(m.dev.deselects, 1);
            assert_eq!(m.slave.bytes(), 4);
        }
    }

    #[test]
    fn miso_is_released_while_deselected() {
        let mut m = Master::new(dev(), 0);
        assert_eq!(m.slave.drive(), PinDrive::NONE);
        m.select();
        assert_eq!(m.slave.drive().mask, MISO);
        m.deselect();
        assert_eq!(m.slave.drive(), PinDrive::NONE);
    }

    #[test]
    fn a_byte_cut_short_by_cs_is_dropped() {
        let mut m = Master::new(dev(), 0);
        m.select();
        // Four clocks only.
        for _ in 0..4 {
            m.sclk = true;
            m.push();
            m.sclk = false;
            m.push();
        }
        m.deselect();
        m.select();
        assert_eq!(m.byte(0x81), 0xA5, "a fresh transfer starts at bit 7");
        m.deselect();
        assert_eq!(m.dev.seen, vec![0x81]);
    }

    #[test]
    fn clocks_while_deselected_are_ignored() {
        let mut m = Master::new(dev(), 3);
        for _ in 0..20 {
            m.sclk = !m.sclk;
            m.push();
        }
        assert_eq!(m.slave.bytes(), 0);
        assert!(!m.slave.is_selected());
    }
}
