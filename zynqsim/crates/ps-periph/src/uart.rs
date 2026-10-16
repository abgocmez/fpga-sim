//! Cadence UART (xuartps) at 0xE0000000 (UART0, SPI 59) and 0xE0001000 (UART1, SPI 82).
//!
//! Modelled: control and mode registers, the interrupt enable/disable/mask/status set, baud
//! generator and divider, FIFO trigger levels, the 64-byte TX and RX FIFOs at offset 0x30, and
//! the channel status register. Transmit bytes leave the FIFO at the baud rate (10 bits per
//! byte, rate = reference clock / (BAUDGEN x (BAUDDIV + 1))) and are appended to a host sink,
//! so a driver that writes faster than that sees TXFULL, as on hardware. Received bytes are
//! injected by the host. Not modelled: the RX timeout interrupt, the modem lines, break,
//! parity and framing errors, IrDA, and the reference-clock source select.
//!
//! Register offsets and bit positions follow UG585 ch.19 and the xuartps driver as remembered.
//! `VERIFY:` the reset values of BAUDGEN, RXWM and TXWM, and the reference clock.

use crate::clock::CPU_HZ;
use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::VecDeque;

/// Interrupt ID of UART0.
pub const UART0_IRQ: u16 = 59;
/// Interrupt ID of UART1.
pub const UART1_IRQ: u16 = 82;
/// Depth of the transmit and receive FIFOs.
pub const FIFO_DEPTH: usize = 64;

// Registers (offsets).
const CR: u64 = 0x00;
const MR: u64 = 0x04;
const IER: u64 = 0x08;
const IDR: u64 = 0x0C;
const IMR: u64 = 0x10;
const ISR: u64 = 0x14;
const BAUDGEN: u64 = 0x18;
const RXTOUT: u64 = 0x1C;
const RXWM: u64 = 0x20;
const MODEMCR: u64 = 0x24;
const MODEMSR: u64 = 0x28;
const SR: u64 = 0x2C;
const FIFO: u64 = 0x30;
const BAUDDIV: u64 = 0x34;
const FLOWDEL: u64 = 0x38;
const TXWM: u64 = 0x44;

// CR bits.
const CR_RXRST: u32 = 1 << 0;
const CR_TXRST: u32 = 1 << 1;
const CR_RX_EN: u32 = 1 << 2;
const CR_RX_DIS: u32 = 1 << 3;
const CR_TX_EN: u32 = 1 << 4;
const CR_TX_DIS: u32 = 1 << 5;
const CR_RSTTO: u32 = 1 << 6;
const CR_STARTBRK: u32 = 1 << 7;
const CR_STOPBRK: u32 = 1 << 8;

// ISR / IER / IDR / IMR bits.
const IX_RTRIG: u32 = 1 << 0;
const IX_REMPTY: u32 = 1 << 1;
const IX_RFUL: u32 = 1 << 2;
const IX_TEMPTY: u32 = 1 << 3;
const IX_TFUL: u32 = 1 << 4;
const IX_OVER: u32 = 1 << 5;
const IX_TOVR: u32 = 1 << 12;
const IX_MASK: u32 = 0x3FFF;

// SR bits (the status bits mirror the FIFO states).
const SR_RTRIG: u32 = 1 << 0;
const SR_REMPTY: u32 = 1 << 1;
const SR_RFUL: u32 = 1 << 2;
const SR_TEMPTY: u32 = 1 << 3;
const SR_TFUL: u32 = 1 << 4;

/// Bits per transmitted byte: start, 8 data, stop.
const FRAME_BITS: u128 = 10;

/// A Cadence UART with a host-visible transmit log and a host-driven receive path.
pub struct Uart {
    irq: u16,
    tx_dis: bool,
    rx_dis: bool,
    /// CR bits that read back as written: STOPBRK, STARTBRK, RSTTO.
    cr_misc: u32,
    mr: u32,
    imr: u32,
    isr: u32,
    baudgen: u32,
    bauddiv: u32,
    rxtout: u32,
    rxwm: u32,
    txwm: u32,
    modemcr: u32,
    flowdel: u32,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    sink: Vec<u8>,
    ref_hz: u64,
    /// Transmit budget in (reference cycles x CPU_HZ) units, accumulated per tick.
    budget: u128,
    rx_dropped: u64,
}

impl Uart {
    /// A UART in reset state (both directions disabled), raising interrupt `irq`.
    pub fn new(irq: u16) -> Uart {
        Uart {
            irq,
            tx_dis: true,
            rx_dis: true,
            cr_misc: CR_STOPBRK,
            mr: 0,
            imr: 0,
            isr: 0,
            baudgen: 0x28B, // VERIFY: reset value
            bauddiv: 0x0F,
            rxtout: 0,
            rxwm: 0x20, // VERIFY: reset value
            txwm: 0x20, // VERIFY: reset value
            modemcr: 0,
            flowdel: 0,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            sink: Vec::new(),
            ref_hz: crate::clock::UART_REF_HZ,
            budget: 0,
            rx_dropped: 0,
        }
    }

    /// Sets the reference clock the baud rate is derived from, in hertz.
    pub fn with_ref_clock(mut self, hz: u64) -> Uart {
        self.ref_hz = hz;
        self
    }

    /// Every byte transmitted so far, in order.
    pub fn tx_log(&self) -> &[u8] {
        &self.sink
    }

    /// Takes the transmitted bytes, leaving the log empty.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.sink)
    }

    /// Host side: delivers bytes to the receiver. Bytes are dropped (and counted) while
    /// receive is disabled or the FIFO is full. Returns how many were accepted.
    pub fn inject_rx(&mut self, bytes: &[u8]) -> usize {
        let mut accepted = 0;
        for &b in bytes {
            if self.rx_dis {
                self.rx_dropped += 1;
            } else if self.push_rx(b) {
                accepted += 1;
            } else {
                self.rx_dropped += 1;
            }
        }
        accepted
    }

    /// Bytes dropped by the receiver (disabled, or FIFO overrun).
    pub fn rx_dropped(&self) -> u64 {
        self.rx_dropped
    }

    /// Bytes in the transmit FIFO.
    pub fn tx_fifo_len(&self) -> usize {
        self.tx.len()
    }

    /// Bytes in the receive FIFO.
    pub fn rx_fifo_len(&self) -> usize {
        self.rx.len()
    }

    /// Whether the transmit path is enabled.
    pub fn tx_enabled(&self) -> bool {
        !self.tx_dis
    }

    /// Whether the receive path is enabled.
    pub fn rx_enabled(&self) -> bool {
        !self.rx_dis
    }

    /// The baud rate the current BAUDGEN and BAUDDIV give, in bits per second.
    pub fn baud(&self) -> u64 {
        let div = u64::from(self.baudgen) * (u64::from(self.bauddiv) + 1);
        self.ref_hz.checked_div(div).unwrap_or(0)
    }

    fn push_rx(&mut self, b: u8) -> bool {
        if self.rx.len() >= FIFO_DEPTH {
            self.isr |= IX_OVER;
            return false;
        }
        self.rx.push_back(b);
        if self.rx.len() as u32 >= self.rxwm {
            self.isr |= IX_RTRIG;
        }
        if self.rx.len() >= FIFO_DEPTH {
            self.isr |= IX_RFUL;
        }
        true
    }

    fn pop_rx(&mut self) -> Option<u8> {
        let b = self.rx.pop_front();
        if self.rx.is_empty() {
            self.isr |= IX_REMPTY;
        }
        b
    }

    fn push_tx(&mut self, b: u8) {
        if self.tx_dis {
            return;
        }
        if self.tx.len() >= FIFO_DEPTH {
            self.isr |= IX_TOVR;
            return;
        }
        self.tx.push_back(b);
        if self.tx.len() >= FIFO_DEPTH {
            self.isr |= IX_TFUL;
        }
    }

    /// Moves transmit bytes to the sink at the baud rate.
    fn drain(&mut self, cpu_cycles: u64) {
        if self.tx.is_empty() {
            self.budget = 0;
            return;
        }
        let bit_ref_cycles = u128::from(self.baudgen) * (u128::from(self.bauddiv) + 1);
        if bit_ref_cycles == 0 {
            return;
        }
        let frame = bit_ref_cycles * FRAME_BITS * u128::from(CPU_HZ);
        self.budget += u128::from(cpu_cycles) * u128::from(self.ref_hz);
        while self.budget >= frame {
            let Some(b) = self.tx.pop_front() else { break };
            self.sink.push(b);
            self.budget -= frame;
            self.isr |= IX_TEMPTY * u32::from(self.tx.is_empty());
        }
        if self.tx.is_empty() {
            self.budget = 0;
        }
    }

    fn sr(&self) -> u32 {
        let mut s = 0;
        if self.tx.len() >= FIFO_DEPTH {
            s |= SR_TFUL;
        }
        if self.tx.is_empty() {
            s |= SR_TEMPTY;
        }
        if self.rx.len() >= FIFO_DEPTH {
            s |= SR_RFUL;
        }
        if self.rx.is_empty() {
            s |= SR_REMPTY;
        }
        if self.rx.len() as u32 >= self.rxwm {
            s |= SR_RTRIG;
        }
        s
    }

    fn cr(&self) -> u32 {
        let mut c = self.cr_misc;
        if self.tx_dis {
            c |= CR_TX_DIS;
        }
        if self.rx_dis {
            c |= CR_RX_DIS;
        }
        c
    }

    /// Readback of an aligned register.
    fn word(&self, a: u64) -> u32 {
        match a {
            CR => self.cr(),
            MR => self.mr,
            IMR => self.imr,
            ISR => self.isr,
            BAUDGEN => self.baudgen,
            RXTOUT => self.rxtout,
            RXWM => self.rxwm,
            MODEMCR => self.modemcr,
            MODEMSR => 0,
            SR => self.sr(),
            BAUDDIV => self.bauddiv,
            FLOWDEL => self.flowdel,
            TXWM => self.txwm,
            _ => 0,
        }
    }

    fn write_word(&mut self, a: u64, w: u32) {
        match a {
            CR => {
                if w & CR_TX_DIS != 0 {
                    self.tx_dis = true;
                } else if w & CR_TX_EN != 0 {
                    self.tx_dis = false;
                }
                if w & CR_RX_DIS != 0 {
                    self.rx_dis = true;
                } else if w & CR_RX_EN != 0 {
                    self.rx_dis = false;
                }
                if w & CR_TXRST != 0 {
                    self.tx.clear();
                }
                if w & CR_RXRST != 0 {
                    self.rx.clear();
                }
                self.cr_misc = w & (CR_STOPBRK | CR_STARTBRK | CR_RSTTO);
            }
            MR => self.mr = w & 0x3FF,
            IER => self.imr |= w & IX_MASK,
            IDR => self.imr &= !(w & IX_MASK),
            ISR => self.isr &= !(w & IX_MASK),
            BAUDGEN => self.baudgen = w & 0xFFFF,
            RXTOUT => self.rxtout = w & 0xFF,
            RXWM => self.rxwm = w & 0x3F,
            MODEMCR => self.modemcr = w & 0x3F,
            BAUDDIV => self.bauddiv = w & 0xFF,
            FLOWDEL => self.flowdel = w & 0xFFFF,
            TXWM => self.txwm = w & 0x3F,
            _ => {}
        }
    }

    /// Whether the register at aligned offset `a` keeps its value across a partial write.
    fn is_rw(a: u64) -> bool {
        matches!(
            a,
            CR | MR | BAUDGEN | RXTOUT | RXWM | MODEMCR | BAUDDIV | FLOWDEL | TXWM
        )
    }
}

impl Device for Uart {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let a = offset & !3;
        if a == FIFO {
            // Reading the FIFO pops one byte, whatever the access size.
            lane_read(offset, size, 0)?;
            let b = self.pop_rx().unwrap_or(0);
            return lane_read(offset, size, u32::from(b));
        }
        if a > TXWM {
            return Err(BusFault::Unmapped(offset));
        }
        lane_read(offset, size, self.word(a))
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        if a == FIFO {
            lane_merge(offset, size, value, 0)?;
            let lane = (offset & 3) * 8;
            self.push_tx((value >> lane) as u8);
            return Ok(());
        }
        if a > TXWM {
            return Err(BusFault::Unmapped(offset));
        }
        let cur = if Self::is_rw(a) { self.word(a) } else { 0 };
        let (a, w) = lane_merge(offset, size, value, cur)?;
        self.write_word(a, w);
        Ok(())
    }

    fn tick(&mut self, cycles: u64) {
        self.drain(cycles);
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: self.irq,
            cpu: None,
            level: self.isr & self.imr != 0,
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for v in [
            self.cr(),
            self.mr,
            self.imr,
            self.isr,
            self.baudgen,
            self.bauddiv,
            self.rxtout,
            self.rxwm,
            self.txwm,
            self.modemcr,
            self.flowdel,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend(self.tx.iter());
        out.push(0xFF);
        out.extend(self.rx.iter());
        out.push(0xFF);
        out.extend_from_slice(&(self.sink.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.rx_dropped.to_le_bytes());
        out.extend_from_slice(&self.budget.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UART that is enabled for both directions, with the given baud settings.
    fn open() -> Uart {
        let mut u = Uart::new(UART1_IRQ);
        u.write(CR, 4, u64::from(CR_TX_EN | CR_RX_EN)).unwrap();
        u
    }

    #[test]
    fn reset_state_matches_the_driver_expectations() {
        let mut u = Uart::new(UART1_IRQ);
        assert_eq!(
            u.read(CR, 4).unwrap(),
            0x128,
            "TX and RX disabled, STOPBRK set"
        );
        assert_eq!(u.read(BAUDGEN, 4).unwrap(), 0x28B);
        assert_eq!(u.read(BAUDDIV, 4).unwrap(), 0xF);
        assert_eq!(u.read(SR, 4).unwrap(), 0x0A, "TXEMPTY and RXEMPTY");
        assert_eq!(u.read(IMR, 4).unwrap(), 0);
        assert_eq!(u.read(ISR, 4).unwrap(), 0);
    }

    #[test]
    fn writes_before_enable_are_dropped_and_enable_is_per_direction() {
        let mut u = Uart::new(UART1_IRQ);
        u.write(FIFO, 4, u64::from(b'x')).unwrap();
        assert_eq!(u.tx_fifo_len(), 0);
        u.write(CR, 4, u64::from(CR_TX_EN)).unwrap();
        assert!(u.tx_enabled());
        assert!(!u.rx_enabled());
        assert_eq!(u.read(CR, 4).unwrap() & (CR_TX_DIS as u64), 0);
        u.write(CR, 4, u64::from(CR_TX_DIS)).unwrap();
        assert!(!u.tx_enabled());
    }

    #[test]
    fn transmitted_bytes_reach_the_sink_at_the_baud_rate() {
        let mut u = open();
        // Divisor 651 x 16: one bit takes 10416 reference cycles, a byte 104160.
        u.write(BAUDGEN, 4, 651).unwrap();
        u.write(BAUDDIV, 4, 15).unwrap();
        for b in b"ok" {
            u.write(FIFO, 1, u64::from(*b)).unwrap();
        }
        assert_eq!(u.tx_fifo_len(), 2);
        // Reference 100 MHz, CPU 666.67 MHz: 104160 ref cycles = 694,400 CPU cycles per byte.
        u.tick(600_000);
        assert!(u.tx_log().is_empty());
        u.tick(200_000);
        assert_eq!(u.tx_log(), b"o");
        u.tick(800_000);
        assert_eq!(u.tx_log(), b"ok");
        assert_ne!(u.read(SR, 4).unwrap() & 0x08, 0, "TX empty");
        assert_ne!(u.read(ISR, 4).unwrap() & u64::from(IX_TEMPTY), 0);
    }

    #[test]
    fn a_driver_that_outruns_the_fifo_sees_txfull_and_overflow() {
        let mut u = open();
        u.write(BAUDGEN, 4, 651).unwrap();
        for i in 0..70u64 {
            u.write(FIFO, 1, i).unwrap();
        }
        assert_eq!(u.tx_fifo_len(), FIFO_DEPTH);
        assert_ne!(u.read(SR, 4).unwrap() & u64::from(SR_TFUL), 0);
        assert_ne!(u.read(ISR, 4).unwrap() & u64::from(IX_TOVR), 0);
        assert_ne!(u.read(ISR, 4).unwrap() & u64::from(IX_TFUL), 0);
    }

    #[test]
    fn received_bytes_are_read_from_the_fifo_and_raise_the_trigger() {
        let mut u = open();
        u.write(RXWM, 4, 2).unwrap();
        u.write(IER, 4, u64::from(IX_RTRIG)).unwrap();
        assert_eq!(u.inject_rx(b"A"), 1);
        assert_eq!(u.read(SR, 4).unwrap() & 0x02, 0, "one byte: not empty");
        assert_eq!(u.read(SR, 4).unwrap() & 0x01, 0, "below the trigger level");
        assert!(!u.irq_lines_for_test());
        assert_eq!(u.inject_rx(b"B"), 1);
        assert_ne!(u.read(SR, 4).unwrap() & 0x01, 0, "RTRIG at two bytes");
        assert!(u.irq_lines_for_test());
        assert_eq!(u.read(FIFO, 1).unwrap(), u64::from(b'A'));
        assert_eq!(u.read(FIFO, 1).unwrap(), u64::from(b'B'));
        assert_eq!(u.read(FIFO, 1).unwrap(), 0, "empty FIFO reads zero");
        assert!(
            u.irq_lines_for_test(),
            "ISR is sticky until written, so the enabled RTRIG stays asserted"
        );
        u.write(ISR, 4, u64::from(IX_RTRIG)).unwrap();
        assert!(!u.irq_lines_for_test());
    }

    #[test]
    fn isr_is_write_one_to_clear_and_drives_the_line_through_imr() {
        let mut u = open();
        u.write(IER, 4, u64::from(IX_TEMPTY)).unwrap();
        // The empty transmitter raises TEMPTY only after it has sent something.
        u.write(BAUDGEN, 4, 1).unwrap();
        u.write(FIFO, 1, 0x55).unwrap();
        u.tick(10_000_000);
        assert!(u.irq_lines_for_test());
        u.write(ISR, 4, 0).unwrap();
        assert!(u.irq_lines_for_test(), "writing 0 does not clear");
        u.write(ISR, 4, u64::from(IX_TEMPTY)).unwrap();
        assert!(!u.irq_lines_for_test());
        u.write(IER, 4, 0).unwrap();
        u.write(IDR, 4, u64::from(IX_TEMPTY)).unwrap();
        assert_eq!(u.read(IMR, 4).unwrap(), 0);
    }

    #[test]
    fn byte_lanes_of_a_register_are_merged_not_clobbered() {
        let mut u = Uart::new(UART0_IRQ);
        u.write(BAUDGEN, 4, 0x1234).unwrap();
        u.write(BAUDGEN + 1, 1, 0xAB).unwrap();
        assert_eq!(u.read(BAUDGEN, 4).unwrap(), 0xAB34);
        assert_eq!(u.read(BAUDGEN + 1, 1).unwrap(), 0xAB);
    }

    #[test]
    fn disabled_receive_drops_bytes_and_the_count_says_so() {
        let mut u = Uart::new(UART0_IRQ);
        assert_eq!(u.inject_rx(b"xyz"), 0);
        assert_eq!(u.rx_dropped(), 3);
    }

    #[test]
    fn overflow_of_the_receive_fifo_sets_over() {
        let mut u = open();
        let accepted = u.inject_rx(&[0u8; FIFO_DEPTH + 5]);
        assert_eq!(accepted, FIFO_DEPTH);
        assert_eq!(u.rx_dropped(), 5);
        assert_ne!(u.read(ISR, 4).unwrap() & u64::from(IX_OVER), 0);
        assert_ne!(u.read(SR, 4).unwrap() & u64::from(SR_RFUL), 0);
    }

    #[test]
    fn unmapped_offsets_fault() {
        let mut u = Uart::new(UART0_IRQ);
        assert_eq!(u.read(0x48, 4), Err(BusFault::Unmapped(0x48)));
        assert_eq!(u.write(0x48, 4, 0), Err(BusFault::Unmapped(0x48)));
        // Reserved registers inside the block read zero and ignore writes.
        assert_eq!(u.read(0x40, 4), Ok(0));
        assert!(u.write(0x40, 4, 0xFFFF).is_ok());
    }

    impl Uart {
        fn irq_lines_for_test(&self) -> bool {
            let mut v = Vec::new();
            self.irq_lines(&mut v);
            v[0].level
        }
    }
}
