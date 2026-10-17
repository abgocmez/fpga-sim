//! Cadence I2C controllers (xiicps, UG585 ch.21) at 0xE0004000 (I2C0, GIC ID 57) and 0xE0005000
//! (I2C1, GIC ID 80).
//!
//! Master transfers. A transfer starts when the Transfer_size register is written with the
//! controller in master mode (CONTROL bit 1). A write transfer sends the bytes queued in the
//! 16-byte data FIFO (DATA) to the address in ADDR; a read transfer (CONTROL bit 0 set) fills
//! the FIFO from the device. The transfer runs to completion in the write that starts it.
//! Each transfer ends with COMP (ISR bit 0); an address or data NACK also sets NACK (ISR bit 2).
//!
//! Devices sit behind the host-side [`I2cPeer`] trait. By default no device is attached, so
//! every address is NACKed, as on a bus with nothing on it. A peer answers a whole transfer:
//! `Err(Nack)` means the address (or a data byte) was not acknowledged.
//!
//! Registers (`VERIFY:` offsets and bit positions from UG585 ch.21 as remembered; the offsets
//! are the ones `firmware/conf_iicps` uses): CONTROL 0x00 (RW 0, MS 1, NEA 2, ACKEN 3, HOLD 4,
//! CLR_FIFO 6, self clearing), STATUS 0x04 (read only, 0 here), ADDR 0x08 (10 bits), DATA 0x0C,
//! ISR 0x10 (write 1 clears), TRANS_SIZE 0x14, SLV_PAUSE 0x18, TIMEOUT 0x1C (reset 0x1F), IMR 0x20
//! (read only, 1 = masked), IER 0x24 and IDR 0x28.

use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::VecDeque;

/// GIC interrupt IDs: I2C0 and I2C1.
pub const I2C_IRQS: [u16; 2] = [57, 80];
/// Depth of the data FIFO, in bytes.
pub const FIFO_DEPTH: usize = 16;

const CONTROL: u64 = 0x00;
const STATUS: u64 = 0x04;
const ADDR: u64 = 0x08;
const DATA: u64 = 0x0C;
const ISR: u64 = 0x10;
const TRANS_SIZE: u64 = 0x14;
const SLV_PAUSE: u64 = 0x18;
const TIMEOUT: u64 = 0x1C;
const IMR: u64 = 0x20;
const IER: u64 = 0x24;
const IDR: u64 = 0x28;

const CTRL_RW: u32 = 1 << 0;
const CTRL_MS: u32 = 1 << 1;
const CTRL_STORED: u32 = 0x1F;
const CTRL_CLR_FIFO: u32 = 1 << 6;

const IXR_COMP: u32 = 1 << 0;
const IXR_DATA: u32 = 1 << 1;
const IXR_NACK: u32 = 1 << 2;
const IXR_ALL: u32 = 0x2FF;

/// The device did not acknowledge the address or a byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nack;

/// A device (or a bus full of them) behind an I2C controller, answering whole transfers.
pub trait I2cPeer: Send {
    /// A master write of `data` to the 7-bit address `addr`.
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack>;
    /// A master read of `len` bytes from `addr`.
    fn read(&mut self, addr: u8, len: usize) -> Result<Vec<u8>, Nack>;
}

/// An empty bus: every address is NACKed.
pub struct NoDevices;

impl I2cPeer for NoDevices {
    fn write(&mut self, _addr: u8, _data: &[u8]) -> Result<(), Nack> {
        Err(Nack)
    }
    fn read(&mut self, _addr: u8, _len: usize) -> Result<Vec<u8>, Nack> {
        Err(Nack)
    }
}

/// One I2C controller.
pub struct I2c {
    block: usize,
    control: u32,
    addr: u32,
    isr: u32,
    mask: u32,
    trans_size: u32,
    slv_pause: u32,
    timeout: u32,
    fifo: VecDeque<u8>,
    peer: Box<dyn I2cPeer>,
}

impl I2c {
    /// Controller 0 or 1 (other values give 1), in reset state with no device attached.
    pub fn new(block: usize) -> I2c {
        I2c {
            block: block.min(1),
            control: 0,
            addr: 0,
            isr: 0,
            mask: IXR_ALL,
            trans_size: 0,
            slv_pause: 0,
            timeout: 0x1F,
            fifo: VecDeque::new(),
            peer: Box::new(NoDevices),
        }
    }

    /// Attaches the devices of this bus (host side).
    pub fn set_peer(&mut self, peer: Box<dyn I2cPeer>) {
        self.peer = peer;
    }

    /// Bytes in the data FIFO.
    pub fn fifo_len(&self) -> usize {
        self.fifo.len()
    }

    /// Runs one master transfer of `n` bytes to the address in ADDR.
    fn transfer(&mut self, n: usize) {
        let addr = (self.addr & 0x7F) as u8;
        let result = if self.control & CTRL_RW == 0 {
            let take = n.min(self.fifo.len());
            let data: Vec<u8> = self.fifo.drain(..take).collect();
            self.peer.write(addr, &data)
        } else {
            self.peer.read(addr, n).map(|bytes| {
                for b in bytes {
                    if self.fifo.len() < FIFO_DEPTH {
                        self.fifo.push_back(b);
                    }
                }
            })
        };
        self.isr |= IXR_COMP;
        match result {
            Ok(()) => {
                if self.control & CTRL_RW != 0 && !self.fifo.is_empty() {
                    self.isr |= IXR_DATA;
                }
            }
            Err(Nack) => self.isr |= IXR_NACK,
        }
    }

    fn irq_level(&self) -> bool {
        self.isr & !self.mask & IXR_ALL != 0
    }
}

impl Device for I2c {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let word = match offset & !3 {
            CONTROL => self.control,
            STATUS => 0,
            ADDR => self.addr,
            DATA => u32::from(self.fifo.pop_front().unwrap_or(0)),
            ISR => self.isr,
            TRANS_SIZE => self.trans_size,
            SLV_PAUSE => self.slv_pause,
            TIMEOUT => self.timeout,
            IMR => self.mask,
            IER | IDR => 0,
            a if a < 0x1000 => 0,
            _ => return Err(BusFault::Unmapped(offset)),
        };
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        match a {
            CONTROL => {
                let (_, w) = lane_merge(offset, size, value, self.control)?;
                if w & CTRL_CLR_FIFO != 0 {
                    self.fifo.clear();
                }
                self.control = w & CTRL_STORED;
            }
            ADDR => {
                let (_, w) = lane_merge(offset, size, value, self.addr)?;
                self.addr = w & 0x3FF;
            }
            DATA => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                if self.fifo.len() < FIFO_DEPTH {
                    self.fifo.push_back((w & 0xFF) as u8);
                }
            }
            ISR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.isr &= !(w & IXR_ALL);
            }
            TRANS_SIZE => {
                let (_, w) = lane_merge(offset, size, value, self.trans_size)?;
                self.trans_size = w & 0xFF;
                if self.control & CTRL_MS != 0 && self.trans_size > 0 {
                    let n = self.trans_size as usize;
                    self.transfer(n);
                }
            }
            SLV_PAUSE => {
                let (_, w) = lane_merge(offset, size, value, self.slv_pause)?;
                self.slv_pause = w & 0xFF;
            }
            TIMEOUT => {
                let (_, w) = lane_merge(offset, size, value, self.timeout)?;
                self.timeout = w & 0xFF;
            }
            IER => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.mask &= !(w & IXR_ALL);
            }
            IDR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.mask |= w & IXR_ALL;
            }
            STATUS | IMR => {
                lane_merge(offset, size, value, 0)?; // read only
            }
            a if a < 0x1000 => {
                lane_merge(offset, size, value, 0)?;
            }
            _ => return Err(BusFault::Unmapped(offset)),
        }
        Ok(())
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: I2C_IRQS[self.block],
            cpu: None,
            level: self.irq_level(),
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for v in [
            self.control,
            self.addr,
            self.isr,
            self.mask,
            self.trans_size,
            self.slv_pause,
            self.timeout,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.fifo.len() as u32).to_le_bytes());
        out.extend(self.fifo.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn wr(i: &mut I2c, off: u64, v: u32) {
        i.write(off, 4, u64::from(v)).unwrap();
    }

    fn rd(i: &mut I2c, off: u64) -> u32 {
        i.read(off, 4).unwrap() as u32
    }

    /// A device at one address that stores what it is written and answers reads with `fill`.
    struct Eeprom {
        addr: u8,
        stored: Arc<Mutex<Vec<u8>>>,
        fill: u8,
    }

    impl I2cPeer for Eeprom {
        fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
            if addr != self.addr {
                return Err(Nack);
            }
            self.stored.lock().unwrap().extend_from_slice(data);
            Ok(())
        }
        fn read(&mut self, addr: u8, len: usize) -> Result<Vec<u8>, Nack> {
            if addr != self.addr {
                return Err(Nack);
            }
            Ok(vec![self.fill; len])
        }
    }

    #[test]
    fn reset_values_match_the_remembered_ug585() {
        let mut i = I2c::new(0);
        assert_eq!(rd(&mut i, CONTROL), 0);
        assert_eq!(rd(&mut i, ISR), 0);
        assert_eq!(rd(&mut i, IMR), IXR_ALL);
        assert_eq!(rd(&mut i, TIMEOUT), 0x1F);
    }

    #[test]
    fn a_write_to_an_absent_address_is_nacked_and_a_read_too() {
        let mut i = I2c::new(0);
        wr(&mut i, CONTROL, CTRL_MS);
        wr(&mut i, ADDR, 0x50);
        wr(&mut i, DATA, 0xA5);
        wr(&mut i, TRANS_SIZE, 1);
        assert_eq!(rd(&mut i, ISR) & IXR_NACK, IXR_NACK);
        assert_eq!(rd(&mut i, ISR) & IXR_COMP, IXR_COMP);
        assert_eq!(i.fifo_len(), 0, "the sent byte is consumed");
        wr(&mut i, ISR, IXR_NACK);
        assert_eq!(rd(&mut i, ISR) & IXR_NACK, 0, "write 1 clears");
        wr(&mut i, CONTROL, CTRL_MS | CTRL_RW);
        wr(&mut i, TRANS_SIZE, 1);
        assert_eq!(rd(&mut i, ISR) & IXR_NACK, IXR_NACK);
    }

    #[test]
    fn a_write_transfer_reaches_an_attached_device() {
        let stored = Arc::new(Mutex::new(Vec::new()));
        let mut i = I2c::new(1);
        i.set_peer(Box::new(Eeprom {
            addr: 0x50,
            stored: Arc::clone(&stored),
            fill: 0,
        }));
        wr(&mut i, CONTROL, CTRL_MS);
        wr(&mut i, ADDR, 0x50);
        wr(&mut i, DATA, 0x12);
        wr(&mut i, DATA, 0x34);
        wr(&mut i, TRANS_SIZE, 2);
        assert_eq!(*stored.lock().unwrap(), vec![0x12, 0x34]);
        assert_eq!(rd(&mut i, ISR) & IXR_NACK, 0);
    }

    #[test]
    fn a_read_transfer_fills_the_fifo_from_the_device() {
        let stored = Arc::new(Mutex::new(Vec::new()));
        let mut i = I2c::new(0);
        i.set_peer(Box::new(Eeprom {
            addr: 0x51,
            stored,
            fill: 0x7E,
        }));
        wr(&mut i, CONTROL, CTRL_MS | CTRL_RW | CTRL_CLR_FIFO);
        wr(&mut i, ADDR, 0x51);
        wr(&mut i, TRANS_SIZE, 3);
        assert_eq!(i.fifo_len(), 3);
        assert_eq!(rd(&mut i, DATA), 0x7E);
        assert_eq!(rd(&mut i, DATA), 0x7E);
    }

    #[test]
    fn ier_and_idr_move_the_mask_and_imr_is_read_only() {
        let mut i = I2c::new(0);
        wr(&mut i, IER, IXR_COMP);
        let enabled = rd(&mut i, IMR) & IXR_COMP;
        wr(&mut i, IDR, IXR_COMP);
        let disabled = rd(&mut i, IMR) & IXR_COMP;
        assert_eq!(enabled | disabled, IXR_COMP);
        wr(&mut i, IMR, 0);
        assert_eq!(rd(&mut i, IMR), IXR_ALL);
    }

    #[test]
    fn the_interrupt_follows_status_and_mask_on_the_right_line() {
        let mut i = I2c::new(1);
        wr(&mut i, CONTROL, CTRL_MS);
        wr(&mut i, ADDR, 0x33);
        wr(&mut i, TRANS_SIZE, 1);
        let mut lines = Vec::new();
        i.irq_lines(&mut lines);
        assert!(!lines[0].level, "masked");
        wr(&mut i, IER, IXR_NACK);
        lines.clear();
        i.irq_lines(&mut lines);
        assert_eq!(
            lines[0],
            IrqInput {
                id: 80,
                cpu: None,
                level: true
            }
        );
    }
}
