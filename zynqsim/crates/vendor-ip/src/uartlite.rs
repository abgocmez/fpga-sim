//! AXI UART Lite (`xilinx.com:ip:axi_uartlite:2.0`, Xilinx PG142), register level.
//!
//! Register map (recalled from PG142, **not** checked against hardware):
//!
//! | offset | register | access |
//! |---|---|---|
//! | `0x00` | `RX_FIFO` | R |
//! | `0x04` | `TX_FIFO` | W |
//! | `0x08` | `STAT_REG` | R |
//! | `0x0C` | `CTRL_REG` | W |
//!
//! `STAT_REG` bits: 0 RX FIFO valid data, 1 RX FIFO full, 2 TX FIFO empty, 3 TX FIFO full,
//! 4 interrupt enabled, 5 overrun error, 6 frame error, 7 parity error. Reset value `0x4`.
//! `CTRL_REG` bits: 0 reset TX FIFO, 1 reset RX FIFO, 4 enable interrupt.
//!
//! Modelled: 16-deep FIFOs, a character shifter paced by `C_BAUDRATE` and the frame format
//! (`C_DATA_BITS`, `C_USE_PARITY`), receive bytes injected by the host (paced the same way), the
//! overrun flag, the interrupt. Transmitted bytes are collected with timestamps and reach the
//! host through the engine (`uart_read` port 2 + channel). There are no pins: frame and parity
//! errors never occur.
//!
//! Interrupt model: the output is `enabled && pending`. `pending` is set when the RX FIFO goes
//! from empty to non-empty, when the TX FIFO becomes empty (the last character moved to the
//! shifter), and when the interrupt is enabled while either condition holds; it is cleared by a
//! read of `STAT_REG`.
//!
//! `VERIFY:` items:
//! * The interrupt condition and what clears it (above): the real IP is documented as an
//!   interrupt on those two events; whether a `STAT_REG` read is what lowers it is a modelling
//!   choice that matches the Xilinx driver's handler (it reads `STAT_REG` first) but is not
//!   confirmed.
//! * "TX FIFO empty" means the FIFO, not the shifter: a character in the shifter does not
//!   count.
//! * Writing the TX FIFO when it is full drops the byte; reading an empty RX FIFO returns 0.
//! * The overrun flag clears on a `STAT_REG` read.
//! * The reset value of `STAT_REG` is `0x4`.
//! * Offsets other than the table's read as 0 and ignore writes.

use crate::params::{Params, flag, ranged};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `RX_FIFO`.
pub const RX_FIFO: u64 = 0x00;
/// `TX_FIFO`.
pub const TX_FIFO: u64 = 0x04;
/// `STAT_REG`.
pub const STAT_REG: u64 = 0x08;
/// `CTRL_REG`.
pub const CTRL_REG: u64 = 0x0C;

/// `STAT_REG`: RX FIFO has data.
pub const STAT_RX_VALID: u32 = 1 << 0;
/// `STAT_REG`: RX FIFO full.
pub const STAT_RX_FULL: u32 = 1 << 1;
/// `STAT_REG`: TX FIFO empty.
pub const STAT_TX_EMPTY: u32 = 1 << 2;
/// `STAT_REG`: TX FIFO full.
pub const STAT_TX_FULL: u32 = 1 << 3;
/// `STAT_REG`: interrupt enabled.
pub const STAT_IE: u32 = 1 << 4;
/// `STAT_REG`: overrun error.
pub const STAT_OVERRUN: u32 = 1 << 5;
/// `CTRL_REG`: reset the TX FIFO.
pub const CTRL_RST_TX: u32 = 1 << 0;
/// `CTRL_REG`: reset the RX FIFO.
pub const CTRL_RST_RX: u32 = 1 << 1;
/// `CTRL_REG`: enable the interrupt.
pub const CTRL_IE: u32 = 1 << 4;

/// FIFO depth.
pub const FIFO_DEPTH: usize = 16;
/// Bytes the host may have queued towards the receiver (paced at the baud rate).
const RX_QUEUE_MAX: usize = 4096;

/// The IP's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UartConfig {
    /// `C_BAUDRATE`.
    pub baud: u32,
    /// `C_DATA_BITS`, 5 to 8.
    pub data_bits: u32,
    /// `C_USE_PARITY`.
    pub parity: bool,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for UartConfig {
    fn default() -> Self {
        UartConfig {
            baud: 9600,
            data_bits: 8,
            parity: false,
            window: 0x1_0000,
        }
    }
}

impl UartConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<UartConfig, String> {
        Ok(UartConfig {
            baud: ranged(p, "C_BAUDRATE", 9600, 110, 10_000_000)? as u32,
            data_bits: ranged(p, "C_DATA_BITS", 8, 5, 8)? as u32,
            parity: flag(p, "C_USE_PARITY", false)?,
            window,
        })
    }

    /// Nanoseconds one character (start, data, parity, stop) takes on the line.
    pub fn char_ns(&self) -> u64 {
        let bits = 1 + u64::from(self.data_bits) + u64::from(self.parity) + 1;
        (bits * 1_000_000_000).div_ceil(u64::from(self.baud))
    }
}

#[derive(Debug)]
struct State {
    cfg: UartConfig,
    tx_fifo: VecDeque<u8>,
    /// The character being shifted out and the time it completes.
    shifter: Option<(u8, u64)>,
    tx_log: Vec<(u64, u8)>,
    rx_queue: VecDeque<(u64, u8)>,
    rx_fifo: VecDeque<u8>,
    rx_last_arrival: u64,
    ie: bool,
    pending: bool,
    overrun: bool,
    now_ns: u64,
}

impl State {
    fn new(cfg: UartConfig) -> State {
        State {
            cfg,
            tx_fifo: VecDeque::new(),
            shifter: None,
            tx_log: Vec::new(),
            rx_queue: VecDeque::new(),
            rx_fifo: VecDeque::new(),
            rx_last_arrival: 0,
            ie: false,
            pending: false,
            overrun: false,
            now_ns: 0,
        }
    }

    /// Moves the head of the TX FIFO into the shifter at `at`.
    fn start_next(&mut self, at: u64) {
        if let Some(b) = self.tx_fifo.pop_front() {
            self.shifter = Some((b, at + self.cfg.char_ns()));
            if self.tx_fifo.is_empty() {
                self.pending = true;
            }
        }
    }

    fn sync(&mut self, now_ns: u64) {
        if now_ns <= self.now_ns {
            return;
        }
        loop {
            // The next thing to happen no later than `now_ns`: a character leaving the shifter
            // or a byte arriving at the receiver.
            let tx_t = self.shifter.map(|(_, t)| t).filter(|&t| t <= now_ns);
            let rx_t = self
                .rx_queue
                .front()
                .map(|&(t, _)| t)
                .filter(|&t| t <= now_ns);
            match (tx_t, rx_t) {
                (Some(t), r) if r.is_none_or(|r| t <= r) => {
                    if let Some((b, _)) = self.shifter.take() {
                        self.tx_log.push((t, b));
                    }
                    self.start_next(t);
                }
                (_, Some(_)) => {
                    if let Some((_, b)) = self.rx_queue.pop_front() {
                        if self.rx_fifo.len() >= FIFO_DEPTH {
                            self.overrun = true;
                        } else {
                            if self.rx_fifo.is_empty() {
                                self.pending = true;
                            }
                            self.rx_fifo.push_back(b);
                        }
                    }
                }
                _ => break,
            }
        }
        self.now_ns = now_ns;
    }

    fn stat(&self) -> u32 {
        let mut s = 0;
        if !self.rx_fifo.is_empty() {
            s |= STAT_RX_VALID;
        }
        if self.rx_fifo.len() >= FIFO_DEPTH {
            s |= STAT_RX_FULL;
        }
        if self.tx_fifo.is_empty() {
            s |= STAT_TX_EMPTY;
        }
        if self.tx_fifo.len() >= FIFO_DEPTH {
            s |= STAT_TX_FULL;
        }
        if self.ie {
            s |= STAT_IE;
        }
        if self.overrun {
            s |= STAT_OVERRUN;
        }
        s
    }
}

/// An AXI UART Lite instance as a [`RegisterBlock`].
pub struct AxiUartLite {
    name: String,
    st: Arc<Mutex<State>>,
}

/// The host side of the UART: collects what the IP transmits and feeds its receiver.
#[derive(Clone)]
pub struct UartHandle {
    st: Arc<Mutex<State>>,
}

impl AxiUartLite {
    /// A freshly reset instance named `name`.
    pub fn new(name: &str, cfg: UartConfig) -> (AxiUartLite, UartHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxiUartLite {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            UartHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl UartHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Brings the model to `now_ns` and removes the characters that finished transmitting:
    /// `(completion time in ns, byte)`.
    pub fn take_tx(&self, now_ns: u64) -> Vec<(u64, u8)> {
        let mut s = self.lock();
        s.sync(now_ns);
        std::mem::take(&mut s.tx_log)
    }

    /// The host sends `data`; the characters arrive one character time apart starting at
    /// `now_ns`. Returns how many were accepted (the queue towards the receiver is bounded).
    pub fn push_rx(&self, data: &[u8], now_ns: u64) -> usize {
        let mut s = self.lock();
        s.sync(now_ns);
        let ch = s.cfg.char_ns();
        let mut n = 0;
        for &b in data {
            if s.rx_queue.len() >= RX_QUEUE_MAX {
                break;
            }
            let at = s.rx_last_arrival.max(now_ns) + ch;
            s.rx_last_arrival = at;
            s.rx_queue.push_back((at, b));
            n += 1;
        }
        n
    }

    /// Interrupt output level.
    pub fn irq(&self) -> bool {
        let s = self.lock();
        s.ie && s.pending
    }

    /// The configuration.
    pub fn config(&self) -> UartConfig {
        self.lock().cfg.clone()
    }
}

impl RegisterBlock for AxiUartLite {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn advance(&mut self, now_ns: u64) {
        self.lock().sync(now_ns);
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let mut s = self.lock();
        match offset {
            RX_FIFO => u32::from(s.rx_fifo.pop_front().unwrap_or(0)),
            STAT_REG => {
                let v = s.stat();
                s.overrun = false;
                s.pending = false;
                v
            }
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        if strobe & 1 == 0 {
            return;
        }
        let mut s = self.lock();
        match offset {
            TX_FIFO => {
                if s.tx_fifo.len() < FIFO_DEPTH {
                    s.tx_fifo.push_back(value as u8);
                    if s.shifter.is_none() {
                        let at = s.now_ns;
                        s.start_next(at);
                    }
                }
            }
            CTRL_REG => {
                if value & CTRL_RST_TX != 0 {
                    s.tx_fifo.clear();
                }
                if value & CTRL_RST_RX != 0 {
                    s.rx_fifo.clear();
                }
                let ie = value & CTRL_IE != 0;
                if ie && !s.ie && (!s.rx_fifo.is_empty() || s.tx_fifo.is_empty()) {
                    s.pending = true;
                }
                if !ie {
                    s.pending = false;
                }
                s.ie = ie;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        let mut s = self.lock();
        let (cfg, now) = (s.cfg.clone(), s.now_ns);
        *s = State::new(cfg);
        s.now_ns = now;
    }

    fn irq_level(&self) -> bool {
        let s = self.lock();
        s.ie && s.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 Mbaud, 8N1: a character is 10 us.
    fn uart() -> (AxiUartLite, UartHandle) {
        AxiUartLite::new(
            "u",
            UartConfig {
                baud: 1_000_000,
                ..UartConfig::default()
            },
        )
    }

    const CH: u64 = 10_000;

    #[test]
    fn reset_status_and_char_time() {
        let (mut u, _) = uart();
        assert_eq!(u.read32(STAT_REG), STAT_TX_EMPTY);
        assert_eq!(
            UartConfig::default().char_ns(),
            1_041_667,
            "10 bits at 9600"
        );
        let c = UartConfig {
            parity: true,
            baud: 115_200,
            ..UartConfig::default()
        };
        assert_eq!(c.char_ns(), 95_487);
    }

    #[test]
    fn transmit_is_paced_by_the_baud_rate_and_stamped() {
        let (mut u, h) = uart();
        u.advance(1_000);
        for b in b"abc" {
            u.write32(TX_FIFO, u32::from(*b), 0xF);
        }
        // 'a' went to the shifter at once; 'b' and 'c' wait in the FIFO.
        u.advance(1_000);
        assert_eq!(u.read32(STAT_REG) & STAT_TX_EMPTY, 0);
        assert!(h.take_tx(1_000 + CH - 1).is_empty());
        assert_eq!(h.take_tx(1_000 + CH), vec![(1_000 + CH, b'a')]);
        assert_eq!(
            h.take_tx(1_000 + 3 * CH),
            vec![(1_000 + 2 * CH, b'b'), (1_000 + 3 * CH, b'c')]
        );
        u.advance(1_000 + 3 * CH);
        assert_eq!(u.read32(STAT_REG) & STAT_TX_EMPTY, STAT_TX_EMPTY);
    }

    #[test]
    fn tx_fifo_holds_16_and_drops_the_rest() {
        let (mut u, h) = uart();
        u.advance(0);
        // One byte in the shifter, 16 in the FIFO, the rest are dropped.
        for i in 0..20u32 {
            u.write32(TX_FIFO, 0x30 + i, 0xF);
        }
        assert_eq!(u.read32(STAT_REG) & STAT_TX_FULL, STAT_TX_FULL);
        let out = h.take_tx(100 * CH);
        assert_eq!(out.len(), 17);
        assert_eq!(out[16].1, 0x30 + 16);
        // CTRL reset TX FIFO discards queued bytes.
        u.advance(200 * CH);
        for i in 0..5u32 {
            u.write32(TX_FIFO, i, 0xF);
        }
        u.write32(CTRL_REG, CTRL_RST_TX, 0xF);
        assert_eq!(h.take_tx(400 * CH).len(), 1, "only the shifter's byte");
    }

    #[test]
    fn receive_fifo_valid_full_and_overrun() {
        let (mut u, h) = uart();
        assert_eq!(h.push_rx(b"hello", 0), 5);
        u.advance(CH - 1);
        assert_eq!(u.read32(STAT_REG) & STAT_RX_VALID, 0);
        u.advance(2 * CH);
        let s = u.read32(STAT_REG);
        assert_eq!(s & STAT_RX_VALID, STAT_RX_VALID);
        assert_eq!(u.read32(RX_FIFO), u32::from(b'h'));
        assert_eq!(u.read32(RX_FIFO), u32::from(b'e'));
        u.advance(10 * CH);
        let rest: Vec<u32> = (0..3).map(|_| u.read32(RX_FIFO)).collect();
        assert_eq!(rest, vec![108, 108, 111]);
        assert_eq!(u.read32(STAT_REG) & STAT_RX_VALID, 0);
        assert_eq!(u.read32(RX_FIFO), 0, "empty FIFO reads 0");
        // 18 bytes into a 16-deep FIFO: full, then overrun (cleared by reading STAT).
        h.push_rx(&[1; 18], 20 * CH);
        u.advance(50 * CH);
        let s = u.read32(STAT_REG);
        assert_eq!(s & STAT_RX_FULL, STAT_RX_FULL);
        assert_eq!(s & STAT_OVERRUN, STAT_OVERRUN);
        assert_eq!(u.read32(STAT_REG) & STAT_OVERRUN, 0);
        // CTRL reset RX FIFO.
        u.write32(CTRL_REG, CTRL_RST_RX, 0xF);
        assert_eq!(u.read32(STAT_REG) & STAT_RX_VALID, 0);
    }

    #[test]
    fn interrupt_on_rx_data_and_on_tx_empty() {
        let (mut u, h) = uart();
        u.advance(0);
        assert!(!u.irq_level());
        // Enabling with an empty TX FIFO raises the interrupt (TX empty condition).
        u.write32(CTRL_REG, CTRL_IE, 0xF);
        assert_eq!(u.read32(STAT_REG) & STAT_IE, STAT_IE);
        assert!(!u.irq_level(), "the STAT read above cleared it");
        // A received byte raises it again.
        h.push_rx(b"x", 100);
        u.advance(100 + CH);
        assert!(u.irq_level());
        assert!(h.irq());
        let _ = u.read32(STAT_REG);
        assert!(!u.irq_level());
        let _ = u.read32(RX_FIFO);
        // A transmitted byte: written to an idle shifter, FIFO empties at once -> interrupt.
        u.write32(TX_FIFO, 0x41, 0xF);
        assert!(u.irq_level());
        let _ = u.read32(STAT_REG);
        // Disabled: the line is low whatever happens.
        u.write32(CTRL_REG, 0, 0xF);
        u.write32(TX_FIFO, 0x42, 0xF);
        assert!(!u.irq_level());
    }

    #[test]
    fn reset_keeps_time_and_clears_state() {
        let (mut u, h) = uart();
        u.advance(1_000);
        u.write32(TX_FIFO, 1, 0xF);
        u.write32(CTRL_REG, CTRL_IE, 0xF);
        u.reset();
        assert_eq!(u.read32(STAT_REG), STAT_TX_EMPTY);
        assert!(h.take_tx(10 * CH).is_empty());
        assert!(!u.irq_level());
    }

    #[test]
    fn parameters() {
        let p: Params = [
            ("C_BAUDRATE".to_string(), "115200".to_string()),
            ("C_DATA_BITS".to_string(), "9".to_string()),
        ]
        .into_iter()
        .collect();
        assert!(UartConfig::from_params(&p, 0x1000).is_err());
    }
}
