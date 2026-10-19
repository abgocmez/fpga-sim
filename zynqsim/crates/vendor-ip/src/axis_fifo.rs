//! AXI4-Stream FIFO (`xilinx.com:ip:axi_fifo_mm_s`, Xilinx PG080), memory-mapped interface,
//! register level.
//!
//! Register map (offsets from the base address; from PG080, as remembered):
//!
//! | offset | register | description | access | reset |
//! |---|---|---|---|---|
//! | `0x00` | `ISR` | Interrupt Status Register (write 1 to clear) | R/W1C | `0x0000_0000` |
//! | `0x04` | `IER` | Interrupt Enable Register | R/W | `0x0000_0000` |
//! | `0x08` | `TDFR` | Transmit data FIFO reset (key `0xA5`) | W | - |
//! | `0x0C` | `TDFV` | Transmit data FIFO vacancy, in words | R | depth |
//! | `0x10` | `TDFD` | Transmit data FIFO data (one word per write) | W | - |
//! | `0x14` | `TLR` | Transmit length (bytes); writing it commits the packet | W | - |
//! | `0x18` | `RDFR` | Receive data FIFO reset (key `0xA5`) | W | - |
//! | `0x1C` | `RDFO` | Receive data FIFO occupancy, in words | R | `0x0000_0000` |
//! | `0x20` | `RDFD` | Receive data FIFO data (one word per read) | R | - |
//! | `0x24` | `RLR` | Receive length (bytes) of the packet at the head of the FIFO | R | `0x0000_0000` |
//! | `0x28` | `SRR` | Software reset (key `0xA5`) | W | - |
//! | `0x2C` | `TDR` | Transmit destination (TDEST) of the next packet | R/W | `0x0000_0000` |
//! | `0x30` | `RDR` | Receive destination (TDEST) of the packet at the head | R | `0x0000_0000` |
//!
//! The stream interface is modelled as a loopback: the TX side feeds the RX side. Writing `TLR`
//! takes `ceil(TLR / 4)` words off the transmit FIFO and makes them one received packet of `TLR`
//! bytes; the packet's words appear in `RDFO`, its length in `RLR`, and `RDFD` pops them. Both
//! `TC` (transmit complete) and `RC` (receive complete) are raised by a committed packet.
//!
//! Interrupt output: `IRQ = (ISR & IER) != 0`. The FIFO has no global enable bit.
//!
//! `ISR` / `IER` bits used here (bits 25..=31):
//! - bit 31 `RPURE`: receive packet underrun read error (`RDFD` read while the FIFO is empty).
//! - bit 30 `RPORE`: receive packet overrun (a committed packet does not fit the RX FIFO).
//! - bit 29 `RPUE`: receive packet underrun error.
//! - bit 28 `TPOE`: transmit packet overrun (`TDFD` written while the TX FIFO is full).
//! - bit 27 `TC`: transmit complete (a packet was committed).
//! - bit 26 `RC`: receive complete (a packet is available to read).
//! - bit 25 `TSE`: transmit size error (`TLR` asks for more words than the TX FIFO holds).
//!
//! `VERIFY:` items (not checked against hardware or a Xilinx simulation; the bit positions in
//! particular are from memory of the product guide and the standalone driver's naming):
//! * the bit positions of every `ISR` flag, and the reset value of `ISR` (here zero);
//! * `TDFV` reports `depth - occupancy` (words) and resets to the TX depth;
//! * `RDFO` counts the words of all queued packets, `RLR` is the byte length of the head packet;
//! * `TLR = 0` is ignored, and `TLR` larger than the TX occupancy commits nothing (`TSE`);
//! * a packet that does not fit the RX FIFO is dropped as a whole (`RPORE`);
//! * `TLR` reads back as zero and `TDFD`/`RDFD`/`TDFR`/`RDFR`/`SRR` read back as zero;
//! * the interrupt output is level `ISR & IER`, with no global enable;
//! * the transfer is immediate (no AXI or stream timing).
//!
//! Unmapped offsets return zero and ignore writes.

use crate::params::{Params, merge, ranged};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `ISR`: Interrupt Status Register.
pub const ISR: u64 = 0x00;
/// `IER`: Interrupt Enable Register.
pub const IER: u64 = 0x04;
/// `TDFR`: Transmit data FIFO reset.
pub const TDFR: u64 = 0x08;
/// `TDFV`: Transmit data FIFO vacancy (words).
pub const TDFV: u64 = 0x0C;
/// `TDFD`: Transmit data FIFO data.
pub const TDFD: u64 = 0x10;
/// `TLR`: Transmit length register (writing it commits a packet).
pub const TLR: u64 = 0x14;
/// `RDFR`: Receive data FIFO reset.
pub const RDFR: u64 = 0x18;
/// `RDFO`: Receive data FIFO occupancy (words).
pub const RDFO: u64 = 0x1C;
/// `RDFD`: Receive data FIFO data.
pub const RDFD: u64 = 0x20;
/// `RLR`: Receive length register (bytes of the head packet).
pub const RLR: u64 = 0x24;
/// `SRR`: Software reset register.
pub const SRR: u64 = 0x28;
/// `TDR`: Transmit destination register.
pub const TDR: u64 = 0x2C;
/// `RDR`: Receive destination register (destination of the head packet).
pub const RDR: u64 = 0x30;

/// Key written to `TDFR`, `RDFR` and `SRR` to reset (PG080).
pub const RESET_KEY: u32 = 0xA5;

/// ISR bit 31: receive packet underrun read error (VERIFY).
pub const INTR_RPURE: u32 = 1 << 31;
/// ISR bit 30: receive packet overrun (VERIFY).
pub const INTR_RPORE: u32 = 1 << 30;
/// ISR bit 29: receive packet underrun error (VERIFY).
pub const INTR_RPUE: u32 = 1 << 29;
/// ISR bit 28: transmit packet overrun (VERIFY).
pub const INTR_TPOE: u32 = 1 << 28;
/// ISR bit 27: transmit complete (VERIFY).
pub const INTR_TC: u32 = 1 << 27;
/// ISR bit 26: receive complete (VERIFY).
pub const INTR_RC: u32 = 1 << 26;
/// ISR bit 25: transmit size error (VERIFY).
pub const INTR_TSE: u32 = 1 << 25;
/// All interrupt bits this model implements.
pub const INTR_MASK: u32 = 0xFE00_0000;

/// IP configuration parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxisFifoConfig {
    /// Depth of the transmit data FIFO in 32-bit words (default 512).
    pub tx_depth: usize,
    /// Depth of the receive data FIFO in 32-bit words (default 512).
    pub rx_depth: usize,
    /// Address window size in bytes (default 64 KiB).
    pub window: u64,
}

impl Default for AxisFifoConfig {
    fn default() -> Self {
        AxisFifoConfig {
            tx_depth: 512,
            rx_depth: 512,
            window: 0x1_0000,
        }
    }
}

impl AxisFifoConfig {
    /// Builds configuration from the parameter dictionary (`C_TX_FIFO_DEPTH`, `C_RX_FIFO_DEPTH`).
    pub fn from_params(p: &Params, window: u64) -> Result<AxisFifoConfig, String> {
        let tx_depth = ranged(p, "C_TX_FIFO_DEPTH", 512, 16, 131_072)? as usize;
        let rx_depth = ranged(p, "C_RX_FIFO_DEPTH", 512, 16, 131_072)? as usize;
        Ok(AxisFifoConfig {
            tx_depth,
            rx_depth,
            window,
        })
    }
}

/// One packet in the receive FIFO: its words not yet read, its length and its destination.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Packet {
    words_left: usize,
    bytes: u32,
    dest: u32,
}

struct State {
    cfg: AxisFifoConfig,
    isr: u32,
    ier: u32,
    tdr: u32,
    tx: VecDeque<u32>,
    rx: VecDeque<u32>,
    packets: VecDeque<Packet>,
    /// Byte length of every packet committed since start (kept across resets).
    committed: Vec<u32>,
}

impl State {
    fn new(cfg: AxisFifoConfig) -> State {
        State {
            cfg,
            isr: 0,
            ier: 0,
            tdr: 0,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            packets: VecDeque::new(),
            committed: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.isr = 0;
        self.ier = 0;
        self.tdr = 0;
        self.tx.clear();
        self.rx.clear();
        self.packets.clear();
    }

    fn tdfv(&self) -> u32 {
        (self.cfg.tx_depth - self.tx.len()) as u32
    }

    fn rdfo(&self) -> u32 {
        self.rx.len() as u32
    }

    fn rlr(&self) -> u32 {
        self.packets.front().map_or(0, |p| p.bytes)
    }

    fn rdr(&self) -> u32 {
        self.packets.front().map_or(0, |p| p.dest)
    }

    /// Writing `TLR`: moves `ceil(bytes / 4)` words from the TX FIFO into the RX FIFO as one packet.
    fn commit(&mut self, bytes: u32) {
        if bytes == 0 {
            return;
        }
        let words = (bytes as usize).div_ceil(4);
        if words > self.tx.len() {
            self.isr |= INTR_TSE;
            return;
        }
        if self.rx.len() + words > self.cfg.rx_depth {
            // The packet does not fit: drop it (its words leave the TX FIFO with it).
            self.tx.drain(..words);
            self.isr |= INTR_RPORE;
            return;
        }
        self.rx.extend(self.tx.drain(..words));
        self.packets.push_back(Packet {
            words_left: words,
            bytes,
            dest: self.tdr,
        });
        self.committed.push(bytes);
        self.isr |= INTR_TC | INTR_RC;
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
    }
}

/// AXI4-Stream FIFO model as a [`RegisterBlock`].
pub struct AxisFifo {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Clonable handle for host / test inspection.
#[derive(Clone)]
pub struct AxisFifoHandle {
    st: Arc<Mutex<State>>,
}

impl AxisFifo {
    /// Creates a new AXI4-Stream FIFO instance.
    pub fn new(name: &str, cfg: AxisFifoConfig) -> (AxisFifo, AxisFifoHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxisFifo {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            AxisFifoHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl AxisFifoHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Byte lengths of the packets committed since start, in order.
    pub fn committed_packets(&self) -> Vec<u32> {
        self.lock().committed.clone()
    }

    /// Words currently held by the receive FIFO.
    pub fn rx_words(&self) -> usize {
        self.lock().rx.len()
    }
}

impl RegisterBlock for AxisFifo {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let mut s = self.lock();
        match offset {
            ISR => s.isr,
            IER => s.ier,
            TDFV => s.tdfv(),
            RDFO => s.rdfo(),
            RDFD => match s.rx.pop_front() {
                Some(word) => {
                    if let Some(p) = s.packets.front_mut() {
                        p.words_left -= 1;
                        if p.words_left == 0 {
                            s.packets.pop_front();
                        }
                    }
                    word
                }
                None => {
                    s.isr |= INTR_RPURE;
                    0
                }
            },
            RLR => s.rlr(),
            TDR => s.tdr,
            RDR => s.rdr(),
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        match offset {
            ISR => {
                // Write 1 to clear.
                let clear = merge(0, value, strobe) & INTR_MASK;
                s.isr &= !clear;
            }
            IER => {
                let cur = s.ier;
                s.ier = merge(cur, value, strobe) & INTR_MASK;
            }
            TDFR => {
                if merge(0, value, strobe) == RESET_KEY {
                    s.tx.clear();
                }
            }
            TDFD => {
                let word = merge(0, value, strobe);
                if s.tx.len() < s.cfg.tx_depth {
                    s.tx.push_back(word);
                } else {
                    s.isr |= INTR_TPOE;
                }
            }
            TLR => {
                let bytes = merge(0, value, strobe);
                s.commit(bytes);
            }
            RDFR => {
                if merge(0, value, strobe) == RESET_KEY {
                    s.rx.clear();
                    s.packets.clear();
                }
            }
            SRR => {
                if merge(0, value, strobe) == RESET_KEY {
                    s.reset();
                }
            }
            TDR => {
                let cur = s.tdr;
                s.tdr = merge(cur, value, strobe);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn irq_level(&self) -> bool {
        self.lock().irq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(depth: usize) -> AxisFifoConfig {
        AxisFifoConfig {
            tx_depth: depth,
            rx_depth: depth,
            ..AxisFifoConfig::default()
        }
    }

    fn push(f: &mut AxisFifo, words: &[u32]) {
        for &w in words {
            f.write32(TDFD, w, 0xF);
        }
    }

    #[test]
    fn reset_state_and_registers() {
        let (mut f, h) = AxisFifo::new("fifo0", AxisFifoConfig::default());
        assert_eq!(f.read32(ISR), 0);
        assert_eq!(f.read32(IER), 0);
        assert_eq!(f.read32(TDFV), 512);
        assert_eq!(f.read32(RDFO), 0);
        assert_eq!(f.read32(RLR), 0);
        assert_eq!(f.read32(TDR), 0);
        assert_eq!(f.read32(RDR), 0);
        assert!(h.committed_packets().is_empty());
        assert!(!f.irq_level());
    }

    #[test]
    fn isr_is_write_one_to_clear() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[1]);
        f.write32(TLR, 4, 0xF);
        assert_eq!(f.read32(ISR), INTR_TC | INTR_RC);
        f.write32(ISR, INTR_TC, 0xF);
        assert_eq!(f.read32(ISR), INTR_RC);
        f.write32(ISR, INTR_RC, 0xF);
        assert_eq!(f.read32(ISR), 0);
    }

    #[test]
    fn ier_masks_the_interrupt_output() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[7]);
        f.write32(TLR, 4, 0xF);
        assert!(!f.irq_level(), "no enable, no interrupt");
        f.write32(IER, INTR_TC, 0xF);
        assert_eq!(f.read32(IER), INTR_TC);
        assert!(f.irq_level());
        f.write32(IER, 0, 0xF);
        assert!(!f.irq_level());
        assert_eq!(
            f.read32(ISR),
            INTR_TC | INTR_RC,
            "status is kept while disabled"
        );
    }

    #[test]
    fn tdfr_key_resets_the_transmit_fifo() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[1, 2, 3]);
        assert_eq!(f.read32(TDFV), 13);
        f.write32(TDFR, 0x1, 0xF);
        assert_eq!(f.read32(TDFV), 13, "wrong key is ignored");
        f.write32(TDFR, RESET_KEY, 0xF);
        assert_eq!(f.read32(TDFV), 16);
    }

    #[test]
    fn tdfd_overflow_sets_tpoe() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[0; 16]);
        assert_eq!(f.read32(TDFV), 0);
        assert_eq!(f.read32(ISR) & INTR_TPOE, 0);
        push(&mut f, &[0xDEAD]);
        assert_eq!(f.read32(ISR) & INTR_TPOE, INTR_TPOE);
        assert_eq!(f.read32(TDFV), 0, "the extra word is not stored");
    }

    #[test]
    fn tlr_commits_a_packet_to_the_receive_side() {
        let (mut f, h) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[0x1111_1111, 0x2222_2222]);
        f.write32(TLR, 7, 0xF);
        assert_eq!(f.read32(TDFV), 16, "transmit words were consumed");
        assert_eq!(f.read32(RDFO), 2, "two words, the last one partly used");
        assert_eq!(f.read32(RLR), 7);
        assert_eq!(f.read32(ISR) & (INTR_TC | INTR_RC), INTR_TC | INTR_RC);
        assert_eq!(h.committed_packets(), vec![7]);
        assert_eq!(h.rx_words(), 2);
    }

    #[test]
    fn rdfd_pops_words_in_order_and_rlr_moves_to_the_next_packet() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[1, 2]);
        f.write32(TLR, 8, 0xF);
        push(&mut f, &[3]);
        f.write32(TLR, 3, 0xF);
        assert_eq!(f.read32(RDFO), 3);
        assert_eq!(f.read32(RLR), 8);
        assert_eq!(f.read32(RDFD), 1);
        assert_eq!(f.read32(RDFD), 2);
        assert_eq!(f.read32(RLR), 3, "the second packet is now at the head");
        assert_eq!(f.read32(RDFO), 1);
        assert_eq!(f.read32(RDFD), 3);
        assert_eq!(f.read32(RLR), 0);
        assert_eq!(f.read32(RDFO), 0);
    }

    #[test]
    fn rdfd_on_an_empty_fifo_is_an_underrun() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        assert_eq!(f.read32(RDFD), 0);
        assert_eq!(f.read32(ISR) & INTR_RPURE, INTR_RPURE);
    }

    #[test]
    fn rdfr_key_drops_the_received_packets() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[9, 9]);
        f.write32(TLR, 8, 0xF);
        f.write32(RDFR, 0x5A, 0xF);
        assert_eq!(f.read32(RDFO), 2, "wrong key is ignored");
        f.write32(RDFR, RESET_KEY, 0xF);
        assert_eq!(f.read32(RDFO), 0);
        assert_eq!(f.read32(RLR), 0);
    }

    #[test]
    fn tlr_larger_than_the_transmit_data_sets_tse_and_commits_nothing() {
        let (mut f, h) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[1]);
        f.write32(TLR, 8, 0xF);
        assert_eq!(f.read32(ISR) & INTR_TSE, INTR_TSE);
        assert_eq!(f.read32(TDFV), 15, "the word stays in the FIFO");
        assert_eq!(f.read32(RDFO), 0);
        assert!(h.committed_packets().is_empty());
    }

    #[test]
    fn a_packet_that_does_not_fit_the_receive_fifo_is_dropped() {
        let (mut f, h) = AxisFifo::new("fifo0", small(16));
        push(&mut f, &[0; 16]);
        f.write32(TLR, 64, 0xF);
        assert_eq!(f.read32(RDFO), 16);
        push(&mut f, &[0; 16]);
        f.write32(TLR, 64, 0xF);
        assert_eq!(f.read32(ISR) & INTR_RPORE, INTR_RPORE);
        assert_eq!(f.read32(RDFO), 16, "the second packet is not stored");
        assert_eq!(h.committed_packets(), vec![64]);
    }

    #[test]
    fn tdr_is_the_destination_of_the_next_packet() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        f.write32(TDR, 5, 0xF);
        assert_eq!(f.read32(TDR), 5);
        push(&mut f, &[1]);
        f.write32(TLR, 4, 0xF);
        f.write32(TDR, 9, 0xF);
        push(&mut f, &[2]);
        f.write32(TLR, 4, 0xF);
        assert_eq!(f.read32(RDR), 5, "the head packet keeps its destination");
        f.read32(RDFD);
        assert_eq!(f.read32(RDR), 9);
    }

    #[test]
    fn srr_key_resets_every_register_and_fifo() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        f.write32(IER, INTR_TC, 0xF);
        f.write32(TDR, 3, 0xF);
        push(&mut f, &[1, 2]);
        f.write32(TLR, 8, 0xF);
        push(&mut f, &[4]);
        assert_eq!(f.read32(RDFO), 2);
        f.write32(SRR, 0x0A, 0xF);
        assert_eq!(f.read32(RDFO), 2, "wrong key does nothing");
        f.write32(SRR, RESET_KEY, 0xF);
        assert_eq!(f.read32(IER), 0);
        assert_eq!(f.read32(TDR), 0);
        assert_eq!(f.read32(TDFV), 16);
        assert_eq!(f.read32(RDFO), 0);
        assert_eq!(f.read32(ISR), 0);
        assert!(!f.irq_level());
    }

    #[test]
    fn unmapped_offsets_read_zero_and_ignore_writes() {
        let (mut f, _) = AxisFifo::new("fifo0", small(16));
        f.write32(0x100, 0xFFFF_FFFF, 0xF);
        assert_eq!(f.read32(0x100), 0);
        assert_eq!(f.read32(ISR), 0);
    }

    #[test]
    fn config_comes_from_the_vivado_parameters() {
        let mut p = Params::new();
        p.insert("C_TX_FIFO_DEPTH".to_string(), "2048".to_string());
        p.insert("C_RX_FIFO_DEPTH".to_string(), "1024".to_string());
        let cfg = AxisFifoConfig::from_params(&p, 0x1000).unwrap();
        assert_eq!(cfg.tx_depth, 2048);
        assert_eq!(cfg.rx_depth, 1024);
        assert_eq!(cfg.window, 0x1000);
    }
}
