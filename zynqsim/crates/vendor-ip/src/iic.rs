//! AXI IIC (`xilinx.com:ip:axi_iic:2.1`, Xilinx PG090), register level, master only, in the
//! core's **dynamic controller** mode.
//!
//! Register map (offsets from the base address; recalled from PG090, **not** checked against
//! hardware or a Xilinx simulation):
//!
//! | offset | register |
//! |---|---|
//! | `0x01C` | `GIE` global interrupt enable (bit 31) |
//! | `0x020` | `ISR` interrupt status, toggle on write |
//! | `0x028` | `IER` interrupt enable |
//! | `0x040` | `SOFTR` soft reset (write `0xA`) |
//! | `0x100` | `CR` control |
//! | `0x104` | `SR` status |
//! | `0x108` | `TX_FIFO` transmit FIFO (16 deep) |
//! | `0x10C` | `RX_FIFO` receive FIFO (16 deep) |
//! | `0x110` | `ADR` own slave address |
//! | `0x114` | `TX_FIFO_OCY` occupancy minus one |
//! | `0x118` | `RX_FIFO_OCY` occupancy minus one |
//! | `0x11C` | `TEN_ADR` |
//! | `0x120` | `RX_FIFO_PIRQ` receive FIFO programmable depth interrupt (4 bits) |
//! | `0x124` | `GPO` |
//! | `0x128`..`0x144` | `TSUSTA`, `TSUSTO`, `THDSTA`, `TSUDAT`, `TBUF`, `THIGH`, `TLOW`, `THDDAT` timing registers (stored) |
//!
//! `CR`: bit 0 `EN`, bit 1 `TX_FIFO_RST` (self clearing), bit 2 `MSMS`, bit 3 `TX`, bit 4
//! `TXAK`, bit 5 `RSTA`, bit 6 `GC_EN`. `SR`: bit 0 `ABGC`, bit 1 `AAS`, bit 2 `BB`, bit 3
//! `SRW`, bit 4 `TX_FIFO_Full`, bit 5 `RX_FIFO_Full`, bit 6 `RX_FIFO_Empty`, bit 7
//! `TX_FIFO_Empty`. `ISR`: bit 0 arbitration lost, bit 1 TX error / slave transmit complete,
//! bit 2 TX FIFO empty, bit 3 RX FIFO full, bit 4 bus not busy, bit 5 addressed as slave, bit 6
//! not addressed as slave, bit 7 TX FIFO half empty.
//!
//! The dynamic controller: a `TX_FIFO` word carries a byte in bits 7:0, `START` in bit 8 and
//! `STOP` in bit 9. A word with `START` is the address byte (7-bit address `<< 1 | R/W`); the
//! core generates START (a repeated START if the bus is busy) and sends it. After a write
//! address every further word is a data byte; after a read address the next word is the
//! **number of bytes to receive** (bits 7:0). `STOP` on a word ends the transfer after that
//! byte (after the last received byte for a read). Received bytes go to `RX_FIFO`; the last
//! byte of a read is NACKed.
//!
//! The bus is bit-banged on the pins an [`I2cBus`] provides: SCL and SDA transitions with the
//! open-drain wired AND of the master and the slave, at `C_SCL_CLK_FREQ_KHZ` (default 100).
//! [`VendorPl`](crate::VendorPl) binds it to two pins of a Pmod port
//! (`"axi_iic_0.IIC" = "board.pmod_ja"`, pins `PMOD_SCL_BIT`/`PMOD_SDA_BIT`, default 2 and 3,
//! the TMP2 module's SCL and SDA), where the device models of `crates/devices` listen. Without
//! a bound bus nothing answers: every address byte is NACKed.
//!
//! The transfer is carried out when the words are written, in simulated time starting at the
//! write: the pins change with bit-accurate timestamps, and the **results appear at their
//! completion times** (RX FIFO bytes, status and interrupt bits) as time advances.
//!
//! Not modelled (documented limits): **standard controller mode** (`CR.MSMS` and `CR.TX`
//! are stored but start no transfer; use the dynamic words), slave mode and general call
//! (`ADR`, `TEN_ADR`, `GC_EN` stored; the core never gets addressed), 10-bit addressing,
//! arbitration (`ISR` bit 0 never sets), clock stretching, glitch filtering, the timing registers
//! (stored, the bit time comes from `C_SCL_CLK_FREQ_KHZ`), `GPO` pins (stored).
//!
//! `VERIFY:` items:
//! * Dynamic-mode word layout (`START` bit 8, `STOP` bit 9, count word after a read address).
//! * After a NACK on the address or a data byte the core sets `ISR` bit 1, generates STOP and
//!   discards the rest of the TX FIFO.
//! * `RX_FIFO_PIRQ`: `ISR` bit 3 sets when the receive FIFO holds more than `PIRQ` bytes.
//! * `ISR` bit 4 (bus not busy) sets when a STOP completes; it is 0 after reset. `ISR` bit 2
//!   sets when the last queued byte has been sent; bit 7 when the TX FIFO occupancy falls to
//!   8 or fewer.
//! * `CR.TX_FIFO_RST` empties the TX FIFO words that have not been sent; since transfers are
//!   carried out at the write, only words written while `EN` is clear can be flushed.
//! * Words written while `EN` is clear stay in the TX FIFO and are carried out when `EN` is set.
//! * Reading `RX_FIFO` when empty returns 0. A byte arriving at a full FIFO is dropped.
//! * Offsets other than the table's read as 0 and ignore writes.

use crate::params::{Params, merge, ranged};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `GIE`.
pub const GIE: u64 = 0x01C;
/// `ISR`.
pub const ISR: u64 = 0x020;
/// `IER`.
pub const IER: u64 = 0x028;
/// `SOFTR`.
pub const SOFTR: u64 = 0x040;
/// `CR`.
pub const CR: u64 = 0x100;
/// `SR`.
pub const SR: u64 = 0x104;
/// `TX_FIFO`.
pub const TX_FIFO: u64 = 0x108;
/// `RX_FIFO`.
pub const RX_FIFO: u64 = 0x10C;
/// `ADR`.
pub const ADR: u64 = 0x110;
/// `TX_FIFO_OCY`.
pub const TX_FIFO_OCY: u64 = 0x114;
/// `RX_FIFO_OCY`.
pub const RX_FIFO_OCY: u64 = 0x118;
/// `TEN_ADR`.
pub const TEN_ADR: u64 = 0x11C;
/// `RX_FIFO_PIRQ`.
pub const RX_FIFO_PIRQ: u64 = 0x120;
/// `GPO`.
pub const GPO: u64 = 0x124;

/// `CR` bit: enable.
pub const CR_EN: u32 = 1 << 0;
/// `CR` bit: reset the TX FIFO.
pub const CR_TX_FIFO_RST: u32 = 1 << 1;
/// `CR` bit: master/slave select (standard mode, stored).
pub const CR_MSMS: u32 = 1 << 2;
/// `SR` bit: bus busy.
pub const SR_BB: u32 = 1 << 2;
/// `SR` bit: TX FIFO full.
pub const SR_TX_FULL: u32 = 1 << 4;
/// `SR` bit: RX FIFO full.
pub const SR_RX_FULL: u32 = 1 << 5;
/// `SR` bit: RX FIFO empty.
pub const SR_RX_EMPTY: u32 = 1 << 6;
/// `SR` bit: TX FIFO empty.
pub const SR_TX_EMPTY: u32 = 1 << 7;
/// `ISR` bit: arbitration lost.
pub const INT_ARB_LOST: u32 = 1 << 0;
/// `ISR` bit: transmit error.
pub const INT_TX_ERR: u32 = 1 << 1;
/// `ISR` bit: TX FIFO empty.
pub const INT_TX_EMPTY: u32 = 1 << 2;
/// `ISR` bit: RX FIFO full.
pub const INT_RX_FULL: u32 = 1 << 3;
/// `ISR` bit: bus not busy.
pub const INT_BNB: u32 = 1 << 4;
/// `ISR` bit: TX FIFO half empty.
pub const INT_TX_HALF: u32 = 1 << 7;
/// `TX_FIFO` word bit: START.
pub const TX_START: u32 = 1 << 8;
/// `TX_FIFO` word bit: STOP.
pub const TX_STOP: u32 = 1 << 9;

const FIFO_DEPTH: usize = 16;
const ISR_MASK: u32 = 0xFF;

/// The two open-drain lines the master drives. Implemented by the board side.
pub trait I2cBus: Send {
    /// The master drives SCL and SDA (`true` = released/high) at PL time `t_ns`. Returns the
    /// resulting level of SDA: the wired AND of the master and whatever the devices on the bus
    /// pull low.
    fn drive(&mut self, t_ns: u64, scl: bool, sda: bool) -> bool;
}

/// The IP's configuration (`C_*` parameters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IicConfig {
    /// `C_SCL_CLK_FREQ_KHZ`, bus clock in kHz (default 100).
    pub scl_khz: u32,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for IicConfig {
    fn default() -> Self {
        IicConfig {
            scl_khz: 100,
            window: 0x1_0000,
        }
    }
}

impl IicConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<IicConfig, String> {
        Ok(IicConfig {
            scl_khz: ranged(p, "C_SCL_CLK_FREQ_KHZ", 100, 1, 1000)? as u32,
            window,
        })
    }

    fn bit_ns(&self) -> u64 {
        1_000_000 / u64::from(self.scl_khz)
    }
}

/// What happens to the registers when a transfer step is over.
#[derive(Debug)]
enum Eff {
    Rx(u8),
    Isr(u32),
    BusIdle,
    TxWordDone,
}

struct State {
    cfg: IicConfig,
    bus: Option<Box<dyn I2cBus>>,
    cr: u32,
    isr: u32,
    ier: u32,
    gie: bool,
    adr: u32,
    ten_adr: u32,
    gpo: u32,
    timing: [u32; 8],
    rx_pirq: u32,
    /// Words written while `EN` is clear.
    queued: VecDeque<u32>,
    rx: VecDeque<u8>,
    /// Words written and not yet sent (occupancy of the TX FIFO).
    tx_words: u32,
    bb: bool,
    effects: VecDeque<(u64, Eff)>,
    /// Time the bus is free again.
    free_at: u64,
    now: u64,
    // Transfer state.
    in_transfer: bool,
    reading: bool,
    /// Lines as the master last drove them.
    scl: bool,
    sda: bool,
    stats: IicStats,
}

/// Counters for tests and reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IicStats {
    /// START and repeated START conditions generated.
    pub starts: u32,
    /// STOP conditions generated.
    pub stops: u32,
    /// Bytes written (address bytes included).
    pub bytes_out: u32,
    /// Bytes read.
    pub bytes_in: u32,
    /// Bytes the slave did not acknowledge.
    pub nacks: u32,
}

impl State {
    fn power_on(cfg: IicConfig, bus: Option<Box<dyn I2cBus>>) -> State {
        State {
            cfg,
            bus,
            cr: 0,
            isr: 0,
            ier: 0,
            gie: false,
            adr: 0,
            ten_adr: 0,
            gpo: 0,
            timing: [0; 8],
            rx_pirq: 0,
            queued: VecDeque::new(),
            rx: VecDeque::new(),
            tx_words: 0,
            bb: false,
            effects: VecDeque::new(),
            free_at: 0,
            now: 0,
            in_transfer: false,
            reading: false,
            scl: true,
            sda: true,
            stats: IicStats::default(),
        }
    }

    fn reset(&mut self) {
        let cfg = self.cfg.clone();
        let bus = self.bus.take();
        *self = State::power_on(cfg, bus);
    }

    /// Applies the effects whose time has come.
    fn settle(&mut self) {
        while self.effects.front().is_some_and(|(t, _)| *t <= self.now) {
            let Some((_, e)) = self.effects.pop_front() else {
                break;
            };
            match e {
                Eff::Rx(b) => {
                    if self.rx.len() < FIFO_DEPTH {
                        self.rx.push_back(b);
                    }
                    if self.rx.len() as u32 > self.rx_pirq {
                        self.isr |= INT_RX_FULL;
                    }
                }
                Eff::Isr(m) => self.isr |= m,
                Eff::BusIdle => {
                    self.bb = false;
                    self.isr |= INT_BNB;
                }
                Eff::TxWordDone => {
                    let before = self.tx_words;
                    self.tx_words = self.tx_words.saturating_sub(1);
                    if before > 8 && self.tx_words <= 8 {
                        self.isr |= INT_TX_HALF;
                    }
                    if self.tx_words == 0 {
                        self.isr |= INT_TX_EMPTY;
                    }
                }
            }
        }
    }

    fn drive(&mut self, t: u64, scl: bool, sda: bool) -> bool {
        self.scl = scl;
        self.sda = sda;
        match &mut self.bus {
            Some(b) => b.drive(t, scl, sda),
            None => sda,
        }
    }

    /// START (or repeated START) at `t`; returns the time after it.
    fn start(&mut self, t: u64) -> u64 {
        let q = self.cfg.bit_ns() / 4;
        let mut t = t;
        if self.in_transfer {
            // Repeated START: release SDA while SCL is low, raise SCL, then pull SDA.
            self.drive(t, false, true);
            t += q;
            self.drive(t, true, true);
            t += q;
        } else {
            self.drive(t, true, true);
            t += q;
        }
        self.drive(t, true, false);
        t += q;
        self.drive(t, false, false);
        t += q;
        self.stats.starts += 1;
        self.in_transfer = true;
        self.bb = true;
        t
    }

    fn stop(&mut self, t: u64) -> u64 {
        let q = self.cfg.bit_ns() / 4;
        let mut t = t;
        self.drive(t, false, false);
        t += q;
        self.drive(t, true, false);
        t += q;
        self.drive(t, true, true);
        t += q;
        self.stats.stops += 1;
        self.in_transfer = false;
        self.effects.push_back((t, Eff::BusIdle));
        t
    }

    /// Sends one bit `b` (MSB first) and returns the SDA level the master sees at the SCL
    /// rising edge, with the time after the bit.
    fn bit(&mut self, t: u64, b: bool) -> (bool, u64) {
        let q = self.cfg.bit_ns() / 4;
        self.drive(t, false, b);
        let seen = self.drive(t + q, true, b);
        self.drive(t + 3 * q, false, b);
        (seen, t + 4 * q)
    }

    /// Sends a byte and reads the ACK. Returns `(acked, time)`.
    fn send_byte(&mut self, t: u64, byte: u8) -> (bool, u64) {
        let mut t = t;
        for i in (0..8).rev() {
            t = self.bit(t, byte >> i & 1 != 0).1;
        }
        let (sda, t) = self.bit(t, true);
        self.stats.bytes_out += 1;
        if sda {
            self.stats.nacks += 1;
        }
        (!sda, t)
    }

    /// Reads a byte and answers ACK/NACK. Returns `(byte, time)`.
    fn recv_byte(&mut self, t: u64, nack: bool) -> (u8, u64) {
        let mut t = t;
        let mut v = 0u8;
        for _ in 0..8 {
            let (s, nt) = self.bit(t, true);
            v = v << 1 | u8::from(s);
            t = nt;
        }
        t = self.bit(t, nack).1;
        self.stats.bytes_in += 1;
        (v, t)
    }

    /// Carries out the queued TX FIFO words.
    fn pump(&mut self) {
        if self.cr & CR_EN == 0 {
            return;
        }
        while let Some(w) = self.queued.pop_front() {
            self.word(w);
        }
    }

    fn word(&mut self, w: u32) {
        let byte = (w & 0xFF) as u8;
        let mut t = self.now.max(self.free_at);
        if w & TX_START != 0 {
            t = self.start(t);
            let (ack, nt) = self.send_byte(t, byte);
            t = nt;
            self.reading = byte & 1 != 0;
            if !ack {
                self.nack_abort(t);
                return;
            }
            if w & TX_STOP != 0 {
                // START and STOP on one word: an address-only probe.
                t = self.stop(t);
                self.reading = false;
            }
        } else if !self.in_transfer {
            // Data without a START: nothing to send it on.
        } else if self.reading {
            let n = u32::from(byte);
            for i in 0..n {
                let last = i + 1 == n;
                let (b, nt) = self.recv_byte(t, last);
                t = nt;
                self.effects.push_back((t, Eff::Rx(b)));
            }
            if w & TX_STOP != 0 {
                t = self.stop(t);
                self.reading = false;
            }
        } else {
            let (ack, nt) = self.send_byte(t, byte);
            t = nt;
            if !ack {
                self.nack_abort(t);
                return;
            }
            if w & TX_STOP != 0 {
                t = self.stop(t);
            }
        }
        self.free_at = t;
        self.effects.push_back((t, Eff::TxWordDone));
        self.settle();
    }

    /// The slave did not acknowledge: error interrupt, STOP, the rest of the FIFO is dropped.
    fn nack_abort(&mut self, t: u64) {
        let dropped = self.queued.len() as u32;
        self.queued.clear();
        let t = self.stop(t);
        self.reading = false;
        self.free_at = t;
        self.effects.push_back((t, Eff::Isr(INT_TX_ERR)));
        for _ in 0..=dropped {
            self.effects.push_back((t, Eff::TxWordDone));
        }
        self.settle();
    }

    fn sr(&self) -> u32 {
        let mut v = 0;
        if self.bb {
            v |= SR_BB;
        }
        if self.tx_words as usize >= FIFO_DEPTH {
            v |= SR_TX_FULL;
        }
        if self.rx.len() >= FIFO_DEPTH {
            v |= SR_RX_FULL;
        }
        if self.rx.is_empty() {
            v |= SR_RX_EMPTY;
        }
        if self.tx_words == 0 {
            v |= SR_TX_EMPTY;
        }
        v
    }

    fn irq(&self) -> bool {
        self.gie && (self.isr & self.ier & ISR_MASK) != 0
    }
}

/// An AXI IIC instance as a [`RegisterBlock`].
pub struct AxiIic {
    name: String,
    st: Arc<Mutex<State>>,
}

/// A clonable handle to the bus connection and counters of an [`AxiIic`].
#[derive(Clone)]
pub struct IicHandle {
    st: Arc<Mutex<State>>,
}

impl AxiIic {
    /// A freshly reset instance named `name` and the handle to its bus.
    pub fn new(name: &str, cfg: IicConfig) -> (AxiIic, IicHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg, None)));
        (
            AxiIic {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            IicHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IicHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Connects the master to a bus (the board's devices).
    pub fn set_bus(&self, bus: Box<dyn I2cBus>) {
        self.lock().bus = Some(bus);
    }

    /// The counters.
    pub fn stats(&self) -> IicStats {
        self.lock().stats
    }

    /// The instance's configuration.
    pub fn config(&self) -> IicConfig {
        self.lock().cfg.clone()
    }
}

impl RegisterBlock for AxiIic {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let mut s = self.lock();
        s.settle();
        match offset {
            GIE => u32::from(s.gie) << 31,
            ISR => s.isr,
            IER => s.ier,
            CR => s.cr,
            SR => s.sr(),
            RX_FIFO => u32::from(s.rx.pop_front().unwrap_or(0)),
            ADR => s.adr,
            TX_FIFO_OCY => s.tx_words.saturating_sub(1),
            RX_FIFO_OCY => (s.rx.len() as u32).saturating_sub(1),
            TEN_ADR => s.ten_adr,
            RX_FIFO_PIRQ => s.rx_pirq,
            GPO => s.gpo,
            o @ 0x128..=0x144 if o % 4 == 0 => s.timing[((o - 0x128) / 4) as usize],
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        s.settle();
        match offset {
            GIE => s.gie = merge(u32::from(s.gie) << 31, value, strobe) >> 31 != 0,
            ISR => s.isr ^= merge(0, value, strobe) & ISR_MASK,
            IER => s.ier = merge(s.ier, value, strobe) & ISR_MASK,
            SOFTR => {
                if merge(0, value, strobe) & 0xF == 0xA {
                    s.reset();
                }
            }
            CR => {
                let new = merge(s.cr, value, strobe) & 0x7F;
                if new & CR_TX_FIFO_RST != 0 {
                    s.queued.clear();
                    s.tx_words = s
                        .effects
                        .iter()
                        .filter(|(_, e)| matches!(e, Eff::TxWordDone))
                        .count() as u32;
                }
                s.cr = new & !CR_TX_FIFO_RST;
                s.pump();
            }
            TX_FIFO => {
                if (s.tx_words as usize) < FIFO_DEPTH {
                    s.tx_words += 1;
                    s.queued.push_back(merge(0, value, strobe) & 0x3FF);
                    s.pump();
                }
            }
            ADR => s.adr = merge(s.adr, value, strobe) & 0xFE,
            TEN_ADR => s.ten_adr = merge(s.ten_adr, value, strobe) & 0x7,
            RX_FIFO_PIRQ => s.rx_pirq = merge(s.rx_pirq, value, strobe) & 0xF,
            GPO => s.gpo = merge(s.gpo, value, strobe),
            o @ 0x128..=0x144 if o % 4 == 0 => {
                let i = ((o - 0x128) / 4) as usize;
                s.timing[i] = merge(s.timing[i], value, strobe);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        let mut s = self.lock();
        s.now = s.now.max(now_ns);
        s.settle();
    }

    fn irq_level(&self) -> bool {
        self.lock().irq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A bus with one slave at `addr` that has a register file, driven by the same I2C pin
    /// state machine the board devices use.
    struct DeviceBus {
        slave: devices::I2cSlave,
        dev: devices::PointerSlave<Regs>,
        log: Arc<Mutex<Vec<u64>>>,
    }

    #[derive(Clone, Debug)]
    struct Regs([u8; 16]);
    impl devices::Registers for Regs {
        fn read(&mut self, reg: u32) -> u8 {
            self.0[(reg as usize) % 16]
        }
        fn write(&mut self, reg: u32, v: u8) {
            self.0[(reg as usize) % 16] = v;
        }
    }

    impl I2cBus for DeviceBus {
        fn drive(&mut self, t_ns: u64, scl: bool, sda: bool) -> bool {
            self.log.lock().unwrap().push(t_ns);
            let levels = u8::from(scl) << 2 | u8::from(sda) << 3;
            self.slave.pins(&mut self.dev, levels);
            sda && self.slave.sda_released()
        }
    }

    fn bus(log: &Arc<Mutex<Vec<u64>>>) -> Box<dyn I2cBus> {
        let mut regs = Regs([0; 16]);
        for (i, v) in regs.0.iter_mut().enumerate() {
            *v = 0xA0 + i as u8;
        }
        Box::new(DeviceBus {
            slave: devices::I2cSlave::new(2, 3),
            dev: devices::PointerSlave::new(0x4B, 1, regs),
            log: Arc::clone(log),
        })
    }

    fn iic(with_bus: bool) -> (AxiIic, IicHandle, Arc<Mutex<Vec<u64>>>) {
        let (b, h) = AxiIic::new("iic", IicConfig::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        if with_bus {
            h.set_bus(bus(&log));
        }
        (b, h, log)
    }

    fn wr(b: &mut AxiIic, off: u64, v: u32) {
        b.write32(off, v, 0xF);
    }

    /// Runs time forward by `us`.
    fn wait(b: &mut AxiIic, t: &mut u64, us: u64) {
        *t += us * 1000;
        b.advance(*t);
    }

    #[test]
    fn reset_state_and_registers() {
        let (mut b, _, _) = iic(false);
        assert_eq!(b.read32(SR), SR_RX_EMPTY | SR_TX_EMPTY);
        assert_eq!(b.read32(CR), 0);
        assert_eq!(b.read32(ISR), 0);
        wr(&mut b, RX_FIFO_PIRQ, 0x1F);
        assert_eq!(b.read32(RX_FIFO_PIRQ), 0xF);
        wr(&mut b, IER, 0xFFFF);
        assert_eq!(b.read32(IER), 0xFF);
        wr(&mut b, GIE, 0x8000_0000);
        assert_eq!(b.read32(GIE), 0x8000_0000);
        wr(&mut b, 0x13C, 77);
        assert_eq!(b.read32(0x13C), 77);
        // Soft reset needs the magic value.
        wr(&mut b, SOFTR, 5);
        assert_eq!(b.read32(IER), 0xFF);
        wr(&mut b, SOFTR, 0xA);
        assert_eq!(b.read32(IER), 0);
        assert_eq!(b.read32(GIE), 0);
    }

    #[test]
    fn isr_toggles_on_write() {
        let (mut b, _, _) = iic(false);
        wr(&mut b, ISR, 0x14);
        assert_eq!(b.read32(ISR), 0x14);
        wr(&mut b, ISR, 0x04);
        assert_eq!(b.read32(ISR), 0x10);
    }

    #[test]
    fn dynamic_write_then_repeated_start_read() {
        let (mut b, h, log) = iic(true);
        wr(&mut b, CR, CR_EN);
        let mut t = 0;
        b.advance(0);
        // Write register pointer 3 then read two bytes: 0xA3, 0xA4.
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1);
        wr(&mut b, TX_FIFO, 3);
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1 | 1);
        wr(&mut b, TX_FIFO, TX_STOP | 2);
        // Nothing has completed yet: the bus is busy and the TX FIFO is not empty.
        assert_eq!(b.read32(SR) & (SR_BB | SR_TX_EMPTY), SR_BB);
        assert_eq!(b.read32(RX_FIFO_OCY), 0);
        assert_eq!(b.read32(TX_FIFO_OCY), 3, "four words queued, occupancy - 1");
        wait(&mut b, &mut t, 100);
        assert_eq!(b.read32(RX_FIFO_OCY), 0);
        wait(&mut b, &mut t, 700);
        let sr = b.read32(SR);
        assert_eq!(
            sr & (SR_BB | SR_TX_EMPTY | SR_RX_EMPTY),
            SR_TX_EMPTY,
            "{sr:#x}"
        );
        assert_eq!(b.read32(RX_FIFO_OCY), 1, "two bytes");
        assert_eq!(b.read32(RX_FIFO), 0xA3);
        assert_eq!(b.read32(RX_FIFO), 0xA4);
        assert_eq!(b.read32(SR) & SR_RX_EMPTY, SR_RX_EMPTY);
        // TX empty and bus-not-busy are flagged, no error.
        let isr = b.read32(ISR);
        assert_eq!(
            isr & (INT_TX_EMPTY | INT_BNB | INT_TX_ERR),
            INT_TX_EMPTY | INT_BNB,
            "{isr:#x}"
        );
        let st = h.stats();
        assert_eq!((st.starts, st.stops, st.nacks), (2, 1, 0));
        assert_eq!((st.bytes_out, st.bytes_in), (3, 2));
        // The pins carry timestamps from the write: strictly non-decreasing, spanning about
        // 6 bytes x 90 us.
        let log = log.lock().unwrap();
        assert!(log.windows(2).all(|w| w[0] <= w[1]));
        assert!(*log.last().unwrap() > 400_000, "{}", log.last().unwrap());
    }

    #[test]
    fn nack_on_an_unknown_address_sets_the_error_and_stops() {
        let (mut b, h, _) = iic(true);
        wr(&mut b, CR, CR_EN);
        wr(&mut b, IER, INT_TX_ERR);
        wr(&mut b, GIE, 0x8000_0000);
        wr(&mut b, TX_FIFO, TX_START | 0x50 << 1);
        wr(&mut b, TX_FIFO, 0x12);
        wr(&mut b, TX_FIFO, TX_STOP | 0x34);
        assert!(
            !b.irq_level(),
            "the error is not there before the byte is over"
        );
        let mut t = 0;
        wait(&mut b, &mut t, 200);
        assert_eq!(b.read32(ISR) & INT_TX_ERR, INT_TX_ERR);
        assert!(b.irq_level());
        assert_eq!(
            b.read32(SR) & (SR_BB | SR_TX_EMPTY),
            SR_TX_EMPTY,
            "stopped, FIFO dropped"
        );
        let st = h.stats();
        assert_eq!((st.nacks, st.stops, st.bytes_out), (1, 1, 1));
        // Acknowledge by toggling.
        wr(&mut b, ISR, INT_TX_ERR);
        assert!(!b.irq_level());
    }

    #[test]
    fn no_bus_means_every_address_is_nacked() {
        let (mut b, _, _) = iic(false);
        wr(&mut b, CR, CR_EN);
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1);
        let mut t = 0;
        wait(&mut b, &mut t, 200);
        assert_ne!(b.read32(ISR) & INT_TX_ERR, 0);
    }

    #[test]
    fn words_wait_for_enable_and_the_fifo_can_be_flushed() {
        let (mut b, h, _) = iic(true);
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1);
        wr(&mut b, TX_FIFO, TX_STOP | 1);
        let mut t = 0;
        wait(&mut b, &mut t, 500);
        assert_eq!(h.stats().starts, 0, "EN is clear");
        assert_eq!(b.read32(SR) & SR_TX_EMPTY, 0);
        wr(&mut b, CR, CR_TX_FIFO_RST);
        assert_eq!(b.read32(SR) & SR_TX_EMPTY, SR_TX_EMPTY);
        assert_eq!(b.read32(CR) & CR_TX_FIFO_RST, 0, "self clearing");
        wr(&mut b, CR, CR_EN);
        wait(&mut b, &mut t, 500);
        assert_eq!(h.stats().starts, 0);
    }

    #[test]
    fn rx_threshold_interrupt_and_tx_half_empty() {
        let (mut b, _, _) = iic(true);
        wr(&mut b, CR, CR_EN);
        wr(&mut b, RX_FIFO_PIRQ, 2);
        // Read 3 bytes from register 0: the third crosses PIRQ = 2.
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1);
        wr(&mut b, TX_FIFO, 0);
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1 | 1);
        wr(&mut b, TX_FIFO, TX_STOP | 3);
        let mut t = 0;
        wait(&mut b, &mut t, 700);
        assert_eq!(b.read32(RX_FIFO_OCY), 2);
        assert_eq!(b.read32(ISR) & INT_RX_FULL, INT_RX_FULL);

        // Fill the TX FIFO with 12 words (address probes are not needed: data to a NACKed
        // address would abort), watch the half-empty flag.
        let (mut b, _, _) = iic(true);
        wr(&mut b, CR, CR_EN);
        wr(&mut b, TX_FIFO, TX_START | 0x4B << 1);
        for _ in 0..11 {
            wr(&mut b, TX_FIFO, 5);
        }
        assert_eq!(b.read32(TX_FIFO_OCY), 11);
        let mut t = 0;
        wait(&mut b, &mut t, 1500);
        assert_eq!(b.read32(ISR) & INT_TX_HALF, INT_TX_HALF);
        assert_eq!(b.read32(SR) & SR_TX_EMPTY, SR_TX_EMPTY);
    }
}
