//! Xilinx CAN controllers (xcanps, UG585 ch.20) at 0xE0008000 (CAN0) and 0xE0009000 (CAN1).
//!
//! Enough of the controller for the standalone BSP's CAN self test (`bsp_canps_polled`), whose
//! register sequence was read from the ELF: `XCanPs_Reset` (SRR = 1), `GetMode` (must read the
//! configuration mode), `SetBaudRatePrescaler` and `SetBitTiming`, `EnterMode(4)` (SRR = 2 with
//! MSR = 2, then GetMode must read 4), four TX FIFO words (0x30..0x3C, the last one queues the
//! frame), then ISR bit 7 must rise and four RX FIFO words (0x50..0x5C) must hold the frame.
//!
//! Mode codes (the values `XCanPs_GetMode` returns, decoded from its disassembly, `VERIFY:` the
//! driver's names): 1 configuration (SRR bit 1 clear), 4 loopback (MSR bit 1), 16 sleep (MSR bit
//! 2), 8 listen (MSR bit 3), 2 normal (MSR 0). The decode reads SR: bit 0 configuration, bit 2
//! listen, bit 3 set unless loopback, bit 12 sleep.
//!
//! Behaviour: in loopback a queued frame is received by the same controller (RX FIFO, ISR bit
//! 7 "RXOK"). In normal mode the frame goes to the host sink ([`Can::take_tx_frames`]) and no
//! acknowledge comes back, so no frame is received. In sleep and listen modes nothing is sent,
//! and in configuration mode the write is dropped. [`Can::inject_rx`] delivers a frame from a
//! host-side node in an operating mode. Reading RX word 3 (0x5C) pops the frame. Frames are four
//! words as the driver writes them; no filtering, error counting or arbitration is modelled.
//!
//! Register offsets from the driver (SRR 0x00, MSR 0x04, BRPR 0x08, BTR 0x0C, ECR 0x10, ESR 0x14,
//! SR 0x18, ISR 0x1C, IER 0x20, ICR 0x24, TX FIFO 0x30..0x3C, RX FIFO 0x50..0x5C), `VERIFY:`
//! against UG585 ch.20. GIC IDs CAN0 = 19 and CAN1 = 46 are `VERIFY:`.

use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::VecDeque;

/// GIC interrupt IDs of CAN0 and CAN1 (VERIFY).
pub const CAN_IRQS: [u16; 2] = [19, 46];

const SRR: u64 = 0x00;
const MSR: u64 = 0x04;
const BRPR: u64 = 0x08;
const BTR: u64 = 0x0C;
const ECR: u64 = 0x10;
const ESR: u64 = 0x14;
const SR: u64 = 0x18;
const ISR: u64 = 0x1C;
const IER: u64 = 0x20;
const ICR: u64 = 0x24;
const TX0: u64 = 0x30;
const RX0: u64 = 0x50;

/// ISR bit 7: a frame was received (loopback or host injection) or transmitted.
const ISR_RXOK: u32 = 1 << 7;
const ISR_MASK: u32 = 0xFFFF;

const MSR_LOOPBACK: u32 = 1 << 1;
const MSR_SLEEP: u32 = 1 << 2;
const MSR_LISTEN: u32 = 1 << 3;
const MSR_MASK: u32 = 0xF;

/// One CAN controller.
pub struct Can {
    block: usize,
    srr: u32,
    msr: u32,
    brpr: u32,
    btr: u32,
    isr: u32,
    ier: u32,
    tx: [u32; 4],
    rx: VecDeque<[u32; 4]>,
    tx_sink: Vec<[u32; 4]>,
}

impl Can {
    /// Controller 0 or 1 (other values give 1), in configuration mode after reset.
    pub fn new(block: usize) -> Can {
        Can {
            block: block.min(1),
            srr: 0,
            msr: 0,
            brpr: 0,
            btr: 0,
            isr: 0,
            ier: 0,
            tx: [0; 4],
            rx: VecDeque::new(),
            tx_sink: Vec::new(),
        }
    }

    fn configuration(&self) -> bool {
        self.srr & 2 == 0
    }

    /// The mode code `XCanPs_GetMode` returns, from the status register.
    fn sr(&self) -> u32 {
        let mut sr = 0;
        if self.configuration() {
            sr |= 1;
        }
        if self.msr & MSR_LISTEN != 0 {
            sr |= 1 << 2;
        }
        if self.msr & MSR_LOOPBACK == 0 {
            sr |= 1 << 3;
        }
        if self.msr & MSR_SLEEP != 0 {
            sr |= 1 << 12;
        }
        sr
    }

    /// Frames the host can take from the normal-mode sink.
    pub fn take_tx_frames(&mut self) -> Vec<[u32; 4]> {
        std::mem::take(&mut self.tx_sink)
    }

    /// Delivers a frame from another node into the RX FIFO. False in configuration mode.
    pub fn inject_rx(&mut self, frame: [u32; 4]) -> bool {
        if self.configuration() {
            return false;
        }
        self.rx.push_back(frame);
        self.isr |= ISR_RXOK;
        true
    }

    /// Frames waiting in the RX FIFO.
    pub fn rx_len(&self) -> usize {
        self.rx.len()
    }

    fn send(&mut self, frame: [u32; 4]) {
        if self.configuration() {
            return;
        }
        if self.msr & MSR_LOOPBACK != 0 {
            self.rx.push_back(frame);
            self.isr |= ISR_RXOK;
        } else if self.msr & (MSR_SLEEP | MSR_LISTEN) == 0 {
            self.tx_sink.push(frame);
        }
    }

    fn reset(&mut self) {
        let block = self.block;
        *self = Can::new(block);
    }
}

impl Device for Can {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let a = offset & !3;
        let word = match a {
            SRR => self.srr,
            MSR => self.msr,
            BRPR => self.brpr,
            BTR => self.btr,
            ECR | ESR => 0,
            SR => self.sr(),
            ISR => self.isr,
            IER => self.ier,
            ICR => 0,
            x if (TX0..TX0 + 16).contains(&x) => self.tx[((x - TX0) / 4) as usize],
            x if (RX0..RX0 + 16).contains(&x) => {
                let k = ((x - RX0) / 4) as usize;
                let frame = self.rx.front().copied().unwrap_or([0; 4]);
                if k == 3 {
                    self.rx.pop_front();
                }
                frame[k]
            }
            x if x < 0x1000 => 0,
            _ => return Err(BusFault::Unmapped(offset)),
        };
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        match a {
            SRR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                if w & 1 != 0 {
                    // SRST: software reset, back to configuration mode.
                    self.reset();
                } else {
                    self.srr = w & 2;
                }
            }
            MSR => {
                let (_, w) = lane_merge(offset, size, value, self.msr)?;
                self.msr = w & MSR_MASK;
            }
            BRPR => {
                let (_, w) = lane_merge(offset, size, value, self.brpr)?;
                self.brpr = w & 0xFF;
            }
            BTR => {
                let (_, w) = lane_merge(offset, size, value, self.btr)?;
                self.btr = w & 0x3FF_FFFF;
            }
            ISR | ICR => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.isr &= !(w & ISR_MASK);
            }
            IER => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.ier |= w & ISR_MASK;
            }
            x if (TX0..TX0 + 16).contains(&x) => {
                let k = ((x - TX0) / 4) as usize;
                let (_, w) = lane_merge(offset, size, value, self.tx[k])?;
                self.tx[k] = w;
                if k == 3 {
                    let frame = self.tx;
                    self.send(frame);
                }
            }
            x if x < 0x1000 => {
                lane_merge(offset, size, value, 0)?;
            }
            _ => return Err(BusFault::Unmapped(offset)),
        }
        Ok(())
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: CAN_IRQS[self.block],
            cpu: None,
            level: self.isr & self.ier != 0,
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for v in [self.srr, self.msr, self.brpr, self.btr, self.isr, self.ier] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.tx {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.rx.len() as u32).to_le_bytes());
        for f in &self.rx {
            for v in f {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wr(c: &mut Can, off: u64, v: u32) {
        c.write(off, 4, u64::from(v)).unwrap();
    }

    fn rd(c: &mut Can, off: u64) -> u32 {
        c.read(off, 4).unwrap() as u32
    }

    /// The mode code `GetMode` would return.
    fn mode(c: &mut Can) -> u32 {
        let sr = rd(c, SR);
        if sr & 1 != 0 {
            1
        } else if sr & 4 != 0 {
            8
        } else if sr & 8 == 0 {
            4
        } else if sr & 0x1000 != 0 {
            16
        } else {
            2
        }
    }

    fn enter(c: &mut Can, msr: u32) {
        wr(c, SRR, 0);
        wr(c, MSR, msr);
        wr(c, SRR, 2);
    }

    #[test]
    fn reset_is_configuration_mode_and_loopback_is_mode_four() {
        let mut c = Can::new(0);
        assert_eq!(mode(&mut c), 1);
        enter(&mut c, MSR_LOOPBACK);
        assert_eq!(mode(&mut c), 4);
        enter(&mut c, 0);
        assert_eq!(mode(&mut c), 2);
        enter(&mut c, MSR_SLEEP);
        assert_eq!(mode(&mut c), 16);
    }

    #[test]
    fn loopback_frame_comes_back_word_for_word_and_is_popped_by_the_last_word() {
        let mut c = Can::new(0);
        enter(&mut c, MSR_LOOPBACK);
        let frame = [0xFA00_0000, 0x8000_0000, 0x1234_5678, 0];
        for (k, w) in frame.iter().enumerate() {
            wr(&mut c, TX0 + 4 * k as u64, *w);
        }
        assert_ne!(rd(&mut c, ISR) & ISR_RXOK, 0, "RXOK rises");
        let got: Vec<u32> = (0..4).map(|k| rd(&mut c, RX0 + 4 * k)).collect();
        assert_eq!(got, frame.to_vec());
        assert_eq!(c.rx_len(), 0, "word 3 pops the frame");
        wr(&mut c, ICR, ISR_RXOK);
        assert_eq!(rd(&mut c, ISR) & ISR_RXOK, 0, "write 1 clears");
    }

    #[test]
    fn normal_mode_sends_to_the_host_and_receives_nothing() {
        let mut c = Can::new(1);
        enter(&mut c, 0);
        for k in 0..4 {
            wr(&mut c, TX0 + 4 * k, 0x100 + k as u32);
        }
        assert_eq!(c.take_tx_frames(), vec![[0x100, 0x101, 0x102, 0x103]]);
        assert_eq!(c.rx_len(), 0);
        assert_eq!(rd(&mut c, ISR) & ISR_RXOK, 0);
    }

    #[test]
    fn sleep_and_configuration_send_nothing() {
        let mut c = Can::new(0);
        for k in 0..4 {
            wr(&mut c, TX0 + 4 * k, 1);
        }
        enter(&mut c, MSR_SLEEP);
        for k in 0..4 {
            wr(&mut c, TX0 + 4 * k, 2);
        }
        assert!(c.take_tx_frames().is_empty());
        assert!(c.inject_rx([1, 2, 3, 4]), "sleep is an operating mode");
        assert_eq!(c.rx_len(), 1);
        wr(&mut c, SRR, 0);
        assert!(!c.inject_rx([1, 2, 3, 4]), "not in an operating mode");
    }

    #[test]
    fn the_interrupt_line_follows_rxok_and_the_enable_register() {
        let mut c = Can::new(0);
        enter(&mut c, MSR_LOOPBACK);
        for k in 0..4 {
            wr(&mut c, TX0 + 4 * k, 0);
        }
        let mut lines = Vec::new();
        c.irq_lines(&mut lines);
        assert!(!lines[0].level, "not enabled");
        wr(&mut c, IER, ISR_RXOK);
        lines.clear();
        c.irq_lines(&mut lines);
        assert_eq!(lines[0], IrqInput { id: 19, cpu: None, level: true });
    }

    #[test]
    fn srst_resets_and_the_registers_keep_their_values() {
        let mut c = Can::new(1);
        wr(&mut c, BRPR, 29);
        wr(&mut c, BTR, 0x0F);
        assert_eq!(rd(&mut c, BRPR), 29);
        wr(&mut c, SRR, 1);
        assert_eq!(rd(&mut c, BRPR), 0);
        assert_eq!(mode(&mut c), 1);
        let mut lines = Vec::new();
        c.irq_lines(&mut lines);
        assert_eq!(lines[0].id, 46);
    }
}
