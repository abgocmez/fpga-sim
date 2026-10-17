//! Xilinx SPI controllers (xspips, UG585 ch.16) at 0xE0006000 (SPI0, GIC ID 58) and 0xE0007000
//! (SPI1, GIC ID 81).
//!
//! Modelled: the Config register (master, clock phase, baud divider, chip select, manual start
//! and its enable, software chip-select force, mode-fail enable), the Enable register, the
//! Delay register, the TX and RX FIFOs (128 words each, TXD and RXD), the TX and RX thresholds,
//! Slave_Idle_count, and the interrupt status, mask, enable and disable registers. IMR reads 1
//! where the interrupt is masked, so IER clears a bit of it and IDR sets one.
//!
//! Transfers. When the controller is a master and enabled and a slave is selected, a transfer
//! moves each TX word through the attached [`SpiPeer`] and pushes the answer into the RX FIFO.
//! With manual start enabled (Config bit 16) the transfer runs on a manual start (Config bit
//! 15, self clearing); otherwise it runs on each TXD write. Nothing is attached by default:
//! [`NoSlave`] answers 0xFF, the pulled-up MISO of an empty bus. [`Loopback`] answers the byte
//! sent, which is how a host test checks the path.
//!
//! Interrupt status bits (ISR): 2 TX FIFO not full (fill below the TX threshold), 3 TX FIFO full
//! (128 words), 4 RX FIFO not empty (fill at least the RX threshold). They are recomputed on
//! every FIFO change; a write of 1 clears them until the next change (`VERIFY:` the latching).
//!
//! `VERIFY:` register offsets, bit positions and reset values as UG585 ch.16 is remembered. The
//! reset value of Config is the one `firmware/conf_spips` expects (MODEFAIL_GEN_EN, bit 17).

use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::VecDeque;

/// GIC interrupt IDs: SPI0 and SPI1.
pub const SPI_IRQS: [u16; 2] = [58, 81];
/// Depth of the TX and RX FIFOs, in words.
pub const FIFO_DEPTH: usize = 128;

const CR: u64 = 0x00;
const ISR: u64 = 0x04;
const IER: u64 = 0x08;
const IDR: u64 = 0x0C;
const IMR: u64 = 0x10;
const ER: u64 = 0x14;
const DR: u64 = 0x18;
const TXD: u64 = 0x1C;
const RXD: u64 = 0x20;
const SICR: u64 = 0x24;
const TXWR: u64 = 0x28;
const RXWR: u64 = 0x2C;

const CR_MSTREN: u32 = 1 << 0;
const CR_MANSTRT: u32 = 1 << 15;
const CR_MANSTRTEN: u32 = 1 << 16;
/// The bits of Config the model stores (MANSTRT is a pulse and is not stored).
const CR_WRITE: u32 = 0x3_FFFF & !CR_MANSTRT;
/// Reset value of Config: MODEFAIL_GEN_EN (VERIFY).
const CR_RESET: u32 = 1 << 17;

const IXR_TXNFULL: u32 = 1 << 2;
const IXR_TXFULL: u32 = 1 << 3;
const IXR_RXNEMPTY: u32 = 1 << 4;
const IXR_ALL: u32 = 0x7F;

/// What a slave answers when the master clocks one byte.
pub trait SpiPeer: Send {
    /// Exchanges one byte with the slave selected by `slave` (0..=3): returns what it sends.
    fn transfer(&mut self, slave: usize, tx: u8) -> u8;
}

/// No slave on the bus: MISO is pulled high.
pub struct NoSlave;

impl SpiPeer for NoSlave {
    fn transfer(&mut self, _slave: usize, _tx: u8) -> u8 {
        0xFF
    }
}

/// A slave that returns each byte it receives (a host-side loopback).
pub struct Loopback;

impl SpiPeer for Loopback {
    fn transfer(&mut self, _slave: usize, tx: u8) -> u8 {
        tx
    }
}

/// One SPI controller.
pub struct Spi {
    block: usize,
    cr: u32,
    isr: u32,
    mask: u32,
    er: u32,
    dr: u32,
    sicr: u32,
    txwr: u32,
    rxwr: u32,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    peer: Box<dyn SpiPeer>,
}

impl Spi {
    /// Controller 0 (SPI0) or 1 (SPI1); other values give controller 1. Reset state: disabled,
    /// nothing attached, every interrupt masked, TX not full.
    pub fn new(block: usize) -> Spi {
        let mut s = Spi {
            block: block.min(1),
            cr: CR_RESET,
            isr: 0,
            mask: IXR_ALL,
            er: 0,
            dr: 0,
            sicr: 0,
            txwr: 1,
            rxwr: 1,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            peer: Box::new(NoSlave),
        };
        s.refresh();
        s
    }

    /// Attaches the slave answering on this bus (host side).
    pub fn set_peer(&mut self, peer: Box<dyn SpiPeer>) {
        self.peer = peer;
    }

    /// Words waiting in the TX FIFO.
    pub fn tx_len(&self) -> usize {
        self.tx.len()
    }

    /// Words waiting in the RX FIFO.
    pub fn rx_len(&self) -> usize {
        self.rx.len()
    }

    /// The slave the chip-select field selects: the lowest active-low bit of CR[13:10].
    fn selected_slave(&self) -> Option<usize> {
        let active = !(self.cr >> 10) & 0xF;
        (active != 0).then(|| active.trailing_zeros() as usize)
    }

    fn can_transfer(&self) -> bool {
        self.er & 1 != 0 && self.cr & CR_MSTREN != 0 && self.selected_slave().is_some()
    }

    /// Clocks every queued TX word through the peer, filling the RX FIFO.
    fn run_transfer(&mut self) {
        let Some(slave) = self.selected_slave() else {
            return;
        };
        while let Some(b) = self.tx.pop_front() {
            let r = self.peer.transfer(slave, b);
            if self.rx.len() < FIFO_DEPTH {
                self.rx.push_back(r);
            }
        }
        self.refresh();
    }

    /// Recomputes the three FIFO status bits from the FIFO fills.
    fn refresh(&mut self) {
        let set = |isr: &mut u32, bit: u32, on: bool| {
            if on {
                *isr |= bit;
            } else {
                *isr &= !bit;
            }
        };
        let (tx, rx) = (self.tx.len(), self.rx.len());
        set(&mut self.isr, IXR_TXNFULL, (tx as u32) < self.txwr);
        set(&mut self.isr, IXR_TXFULL, tx >= FIFO_DEPTH);
        set(&mut self.isr, IXR_RXNEMPTY, (rx as u32) >= self.rxwr.max(1));
    }

    fn irq_level(&self) -> bool {
        self.isr & !self.mask & IXR_ALL != 0
    }
}

impl Device for Spi {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let word = match offset & !3 {
            CR => self.cr,
            ISR => self.isr,
            IMR => self.mask,
            ER => self.er,
            DR => self.dr,
            RXD => {
                // A read pops the RX FIFO; the word is the popped byte.
                let v = self.rx.pop_front().map_or(0, u32::from);
                self.refresh();
                v
            }
            SICR => self.sicr,
            TXWR => self.txwr,
            RXWR => self.rxwr,
            IER | IDR | TXD => 0,
            a if a < 0x1000 => 0,
            _ => return Err(BusFault::Unmapped(offset)),
        };
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        match a {
            CR => {
                let (_, w) = lane_merge(offset, size, value, self.cr)?;
                let pulse = w & CR_MANSTRT != 0;
                self.cr = w & CR_WRITE;
                if pulse && self.cr & CR_MANSTRTEN != 0 && self.can_transfer() {
                    self.run_transfer();
                }
            }
            ISR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.isr &= !(w & IXR_ALL);
            }
            IER => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.mask &= !(w & IXR_ALL);
            }
            IDR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.mask |= w & IXR_ALL;
            }
            IMR => {
                lane_merge(offset, size, value, 0)?; // read-only
            }
            ER => {
                let (_, w) = lane_merge(offset, size, value, self.er)?;
                self.er = w & 1;
            }
            DR => {
                let (_, w) = lane_merge(offset, size, value, self.dr)?;
                self.dr = w;
            }
            TXD => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                if self.tx.len() < FIFO_DEPTH {
                    self.tx.push_back((w & 0xFF) as u8);
                }
                let manual = self.cr & CR_MANSTRTEN != 0;
                if !manual && self.can_transfer() {
                    self.run_transfer();
                }
            }
            RXD => {
                lane_merge(offset, size, value, 0)?;
            }
            SICR => {
                let (_, w) = lane_merge(offset, size, value, self.sicr)?;
                self.sicr = w & 0xFF;
            }
            TXWR => {
                let (_, w) = lane_merge(offset, size, value, self.txwr)?;
                self.txwr = w & 0xFF;
            }
            RXWR => {
                let (_, w) = lane_merge(offset, size, value, self.rxwr)?;
                self.rxwr = w & 0xFF;
            }
            a if a < 0x1000 => {
                lane_merge(offset, size, value, 0)?;
            }
            _ => return Err(BusFault::Unmapped(offset)),
        }
        self.refresh();
        Ok(())
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: SPI_IRQS[self.block],
            cpu: None,
            level: self.irq_level(),
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for v in [self.cr, self.isr, self.mask, self.er, self.dr, self.sicr, self.txwr, self.rxwr] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.tx.len() as u32).to_le_bytes());
        out.extend(self.tx.iter());
        out.extend_from_slice(&(self.rx.len() as u32).to_le_bytes());
        out.extend(self.rx.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wr(s: &mut Spi, off: u64, v: u32) {
        s.write(off, 4, u64::from(v)).unwrap();
    }

    fn rd(s: &mut Spi, off: u64) -> u32 {
        s.read(off, 4).unwrap() as u32
    }

    #[test]
    fn reset_state_has_an_empty_fifo_and_every_interrupt_masked() {
        let mut s = Spi::new(0);
        assert_eq!(rd(&mut s, CR), CR_RESET);
        assert_eq!(rd(&mut s, ISR), IXR_TXNFULL);
        assert_eq!(rd(&mut s, IMR), IXR_ALL);
        assert_eq!(rd(&mut s, TXWR), 1);
    }

    #[test]
    fn manual_start_drains_the_tx_fifo_through_a_loopback_peer() {
        let mut s = Spi::new(0);
        s.set_peer(Box::new(Loopback));
        wr(&mut s, CR, CR_MSTREN | (0b1110 << 10) | CR_MANSTRTEN);
        for i in 0..5u32 {
            wr(&mut s, TXD, i + 1);
        }
        assert_eq!(s.tx_len(), 5, "nothing moves before the start");
        wr(&mut s, ER, 1);
        let cr = rd(&mut s, CR);
        wr(&mut s, CR, cr | CR_MANSTRT);
        assert_eq!(s.tx_len(), 0);
        assert_eq!(s.rx_len(), 5);
        let got: Vec<u32> = (0..5).map(|_| rd(&mut s, RXD)).collect();
        assert_eq!(got, vec![1, 2, 3, 4, 5]);
        assert_eq!(rd(&mut s, CR) & CR_MANSTRT, 0, "the start is self clearing");
    }

    #[test]
    fn without_a_slave_the_answer_is_pulled_high() {
        let mut s = Spi::new(1);
        wr(&mut s, CR, CR_MSTREN | (0b1110 << 10));
        wr(&mut s, ER, 1);
        wr(&mut s, TXD, 0x12);
        assert_eq!(rd(&mut s, RXD), 0xFF);
    }

    #[test]
    fn a_full_fifo_sets_txfull_and_masks_drop_the_line() {
        let mut s = Spi::new(0);
        wr(&mut s, ISR, IXR_ALL);
        for i in 0..FIFO_DEPTH as u32 {
            wr(&mut s, TXD, i);
        }
        assert_ne!(rd(&mut s, ISR) & IXR_TXFULL, 0);
        assert_eq!(rd(&mut s, ISR) & IXR_TXNFULL, 0);
        wr(&mut s, TXD, 0xFF);
        assert_eq!(s.tx_len(), FIFO_DEPTH, "a write to a full FIFO is dropped");
        let mut lines = Vec::new();
        s.irq_lines(&mut lines);
        assert!(!lines[0].level, "every interrupt is masked");
        wr(&mut s, IER, IXR_TXFULL);
        lines.clear();
        s.irq_lines(&mut lines);
        assert_eq!(lines[0], IrqInput { id: 58, cpu: None, level: true });
    }

    #[test]
    fn ier_and_idr_move_the_imr_bits_and_imr_is_read_only() {
        let mut s = Spi::new(0);
        wr(&mut s, IER, IXR_TXNFULL);
        assert_eq!(rd(&mut s, IMR) & IXR_TXNFULL, 0);
        wr(&mut s, IDR, IXR_TXNFULL);
        assert_eq!(rd(&mut s, IMR) & IXR_TXNFULL, IXR_TXNFULL);
        wr(&mut s, IMR, 0);
        assert_eq!(rd(&mut s, IMR), IXR_ALL);
    }

    #[test]
    fn spi1_is_a_separate_controller_with_its_own_interrupt() {
        let s = Spi::new(1);
        let mut lines = Vec::new();
        s.irq_lines(&mut lines);
        assert_eq!(lines[0].id, 81);
    }
}
