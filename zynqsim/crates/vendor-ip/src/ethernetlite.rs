//! Behavioural model of the Xilinx AXI Ethernet Lite 3.0 core (Xilinx PG135).
//!
//! # Overview
//!
//! AXI Ethernet Lite provides a lightweight 10/100 Mb/s Ethernet MAC interfacing directly
//! to an external MII/RMII PHY. Unlike AXI DMA / VDMA, it does not master memory transactions;
//! instead, the CPU reads and writes Ethernet frames directly into internal 2 KiB transmit (TX)
//! and receive (RX) packet buffer memories on `M_AXI_GP`.
//!
//! Address window size is 8 KiB (`0x2000` bytes):
//! * `0x0000` - `0x07E3`: TX Ping Data Buffer (up to 2020 bytes).
//! * `0x07E4`: `TX_PING_LEN` (Bits 11:0: Transmit frame length in bytes).
//! * `0x07E8`: `MDIOADDR` (MDIO Address register: PHY addr, Reg addr, OP).
//! * `0x07EC`: `MDIOWR` (MDIO Write Data register).
//! * `0x07F0`: `MDIORD` (MDIO Read Data register).
//! * `0x07F4`: `MDIOCTRL` (MDIO Control register: Enable, Status/Ready).
//! * `0x07F8`: `GIE` (Global Interrupt Enable Register: bit 31 `GIE`).
//! * `0x07FC`: `TX_PING_CTRL` (TX Ping Control/Status: bit 0 `TX_BUSY`, bit 1 `TX_IE`).
//! * `0x0800` - `0x0FE3`: TX Pong Data Buffer (when `C_INCLUDE_GLOBAL_BUFFERS = 1`).
//! * `0x0FE4`: `TX_PONG_LEN`.
//! * `0x0FFC`: `TX_PONG_CTRL`.
//! * `0x1000` - `0x17E3`: RX Ping Data Buffer.
//! * `0x17E4` / `0x17F4`: `RX_PING_LEN` (Receive frame length in bytes).
//! * `0x17FC`: `RX_PING_CTRL` (RX Ping Control/Status: bit 0 `RX_STATUS`, bit 1 `RX_IE`).
//! * `0x1800` - `0x1FE3`: RX Pong Data Buffer (when `C_INCLUDE_GLOBAL_BUFFERS = 1`).
//! * `0x1FE4` / `0x1FF4`: `RX_PONG_LEN`.
//! * `0x1FFC`: `RX_PONG_CTRL`.
//!
//! Interrupt output `ip2intc_irpt`: single active-high level interrupt line.
//! Asserted when `GIE` is set and any enabled interrupt is active (TX done or RX frame available).
//!
//! `VERIFY:` items (not checked against hardware or Xilinx simulation):
//! * Buffer memory word/byte-lane strobe alignment and max length `0x7E4`;
//! * MDIO auto-negotiation register defaults (PHY link status reports 100M full duplex);
//! * Immediate frame loopback when `C_INCLUDE_INTERNAL_LOOPBACK = 1` or `LOOPBACK = 1`.

use crate::params::{Params, flag, ranged};
use pl_api::PsPort;
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Transmit ping data buffer base offset.
pub const TX_PING_BASE: u64 = 0x0000;
/// Transmit ping buffer length register offset.
pub const TX_PING_LEN: u64 = 0x07E4;
/// MDIO address register offset.
pub const MDIOADDR: u64 = 0x07E8;
/// MDIO write data register offset.
pub const MDIOWR: u64 = 0x07EC;
/// MDIO read data register offset.
pub const MDIORD: u64 = 0x07F0;
/// MDIO control register offset.
pub const MDIOCTRL: u64 = 0x07F4;
/// Global interrupt enable register offset.
pub const GIE: u64 = 0x07F8;
/// Transmit ping control/status register offset.
pub const TX_PING_CTRL: u64 = 0x07FC;

/// Transmit pong data buffer base offset.
pub const TX_PONG_BASE: u64 = 0x0800;
/// Transmit pong buffer length register offset.
pub const TX_PONG_LEN: u64 = 0x0FE4;
/// Transmit pong control/status register offset.
pub const TX_PONG_CTRL: u64 = 0x0FFC;

/// Receive ping data buffer base offset.
pub const RX_PING_BASE: u64 = 0x1000;
/// Receive ping buffer length register offset.
pub const RX_PING_LEN: u64 = 0x17E4;
/// Alternate receive ping buffer length register offset (PG135 table 2-1).
pub const RX_PING_LEN_ALT: u64 = 0x17F4;
/// Receive ping control/status register offset.
pub const RX_PING_CTRL: u64 = 0x17FC;

/// Receive pong data buffer base offset.
pub const RX_PONG_BASE: u64 = 0x1800;
/// Receive pong buffer length register offset.
pub const RX_PONG_LEN: u64 = 0x1FE4;
/// Alternate receive pong buffer length register offset (PG135 table 2-1).
pub const RX_PONG_LEN_ALT: u64 = 0x1FF4;
/// Receive pong control/status register offset.
pub const RX_PONG_CTRL: u64 = 0x1FFC;

/// Buffer capacity in bytes per ping or pong slot (2020 bytes).
pub const BUFFER_SIZE: usize = 0x07E4;

/// Global Interrupt Enable register bit 31.
pub const GIE_ENABLE: u32 = 1 << 31;

/// Transmit Control register bit 0: Transmit Busy / Complete.
pub const TX_CTRL_BUSY: u32 = 1 << 0;
/// Transmit Control register bit 1: Program MAC address.
pub const TX_CTRL_PROGRAM: u32 = 1 << 1;
/// Transmit Control register bit 3: Transmit Interrupt Enable (PG135 Table 2-8).
pub const TX_CTRL_IE: u32 = 1 << 3;
/// Transmit Control register bit 4: Internal loopback enable (PG135 Table 2-8).
pub const TX_CTRL_LOOPBACK: u32 = 1 << 4;

/// Receive Control register bit 0: Receive Status / Complete (1 = frame ready in buffer).
pub const RX_CTRL_STATUS: u32 = 1 << 0;
/// Receive Control register bit 3: Receive Interrupt Enable (PG135 Table 2-10).
pub const RX_CTRL_IE: u32 = 1 << 3;

/// MDIO Control register bit 0: MDIO Enable.
pub const MDIOCTRL_ENABLE: u32 = 1 << 0;
/// MDIO Control register bit 1: MDIO Ready / Status.
pub const MDIOCTRL_READY: u32 = 1 << 1;

/// Static configuration of an AXI Ethernet Lite core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetLiteConfig {
    /// Whether ping-pong double buffering is enabled.
    pub ping_pong: bool,
    /// Whether internal loopback is active (TX frames route to RX).
    pub loopback: bool,
    /// Whether MDIO PHY management interface is enabled.
    pub mdio: bool,
    /// Address window size in bytes (default 8 KiB).
    pub window: u64,
}

impl Default for EthernetLiteConfig {
    fn default() -> Self {
        EthernetLiteConfig {
            ping_pong: false,
            loopback: true,
            mdio: true,
            window: 0x2000,
        }
    }
}

impl EthernetLiteConfig {
    /// Reads configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<EthernetLiteConfig, String> {
        let ping_pong = flag(p, "C_INCLUDE_GLOBAL_BUFFERS", false)?
            || ranged(p, "C_TX_PINGPONG", 0, 0, 1)? != 0
            || ranged(p, "C_RX_PINGPONG", 0, 0, 1)? != 0;
        let loopback = if p.contains_key("C_INCLUDE_INTERNAL_LOOPBACK") {
            flag(p, "C_INCLUDE_INTERNAL_LOOPBACK", true)?
        } else if p.contains_key("LOOPBACK") {
            flag(p, "LOOPBACK", true)?
        } else if p.contains_key("STREAM_LOOPBACK") {
            flag(p, "STREAM_LOOPBACK", true)?
        } else {
            true // Default to loopback for isolated simulation
        };
        let mdio = flag(p, "C_INCLUDE_MDIO", true)?;
        Ok(EthernetLiteConfig {
            ping_pong,
            loopback,
            mdio,
            window: window.max(0x2000),
        })
    }
}

/// Statistics and counters for performance and diagnostic inspection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EthernetLiteStats {
    /// Number of packets transmitted by TX.
    pub tx_packets: u64,
    /// Total bytes transmitted by TX.
    pub tx_bytes: u64,
    /// Number of packets received into RX.
    pub rx_packets: u64,
    /// Total bytes received into RX.
    pub rx_bytes: u64,
    /// Number of received packets dropped due to full RX buffers.
    pub rx_dropped: u64,
}

struct State {
    cfg: EthernetLiteConfig,
    now: u64,
    gie: bool,

    tx_ping_buf: Vec<u8>,
    tx_ping_len: u32,
    tx_ping_ctrl: u32,
    tx_ping_int: bool,

    tx_pong_buf: Vec<u8>,
    tx_pong_len: u32,
    tx_pong_ctrl: u32,
    tx_pong_int: bool,

    rx_ping_buf: Vec<u8>,
    rx_ping_len: u32,
    rx_ping_ctrl: u32,

    rx_pong_buf: Vec<u8>,
    rx_pong_len: u32,
    rx_pong_ctrl: u32,

    mdio_addr: u32,
    mdio_wr: u32,
    mdio_rd: u32,
    mdio_ctrl: u32,
    phy_regs: [u16; 32],

    from_tx: VecDeque<Vec<u8>>,
    stats: EthernetLiteStats,
}

impl State {
    fn new(cfg: EthernetLiteConfig) -> Self {
        let mut phy_regs = [0u16; 32];
        // Standard PHY defaults (100BASE-TX full-duplex, link up)
        phy_regs[0] = 0x1140; // Basic Control: 100 Mbps, Auto-neg, Full Duplex
        phy_regs[1] = 0x786D; // Basic Status: 100BASE-TX capable, Auto-neg complete, Link Up
        phy_regs[2] = 0x0141; // PHY Identifier 1 (e.g. Marvell / generic)
        phy_regs[3] = 0x0DD1; // PHY Identifier 2
        phy_regs[4] = 0x01E1; // Auto-Negotiation Advertisement
        phy_regs[5] = 0x45E1; // Link Partner Ability

        State {
            cfg,
            now: 0,
            gie: false,
            tx_ping_buf: vec![0u8; BUFFER_SIZE],
            tx_ping_len: 0,
            tx_ping_ctrl: 0,
            tx_ping_int: false,
            tx_pong_buf: vec![0u8; BUFFER_SIZE],
            tx_pong_len: 0,
            tx_pong_ctrl: 0,
            tx_pong_int: false,
            rx_ping_buf: vec![0u8; BUFFER_SIZE],
            rx_ping_len: 0,
            rx_ping_ctrl: 0,
            rx_pong_buf: vec![0u8; BUFFER_SIZE],
            rx_pong_len: 0,
            rx_pong_ctrl: 0,
            mdio_addr: 0,
            mdio_wr: 0,
            mdio_rd: 0,
            mdio_ctrl: MDIOCTRL_READY,
            phy_regs,
            from_tx: VecDeque::new(),
            stats: EthernetLiteStats::default(),
        }
    }

    fn reset(&mut self) {
        self.gie = false;
        self.tx_ping_buf.fill(0);
        self.tx_ping_len = 0;
        self.tx_ping_ctrl = 0;
        self.tx_ping_int = false;
        self.tx_pong_buf.fill(0);
        self.tx_pong_len = 0;
        self.tx_pong_ctrl = 0;
        self.tx_pong_int = false;
        self.rx_ping_buf.fill(0);
        self.rx_ping_len = 0;
        self.rx_ping_ctrl = 0;
        self.rx_pong_buf.fill(0);
        self.rx_pong_len = 0;
        self.rx_pong_ctrl = 0;
        self.mdio_addr = 0;
        self.mdio_wr = 0;
        self.mdio_rd = 0;
        self.mdio_ctrl = MDIOCTRL_READY;
        self.from_tx.clear();
    }

    fn irq(&self) -> bool {
        if !self.gie {
            return false;
        }
        let tx_ping_ie = (self.tx_ping_ctrl & TX_CTRL_IE) != 0;
        let tx_pong_ie = (self.tx_pong_ctrl & TX_CTRL_IE) != 0;
        let rx_ping_ie = (self.rx_ping_ctrl & RX_CTRL_IE) != 0;
        let rx_pong_ie = (self.rx_pong_ctrl & RX_CTRL_IE) != 0;

        let tx_irq = (tx_ping_ie && self.tx_ping_int)
            || (self.cfg.ping_pong && tx_pong_ie && self.tx_pong_int);
        let rx_irq = (rx_ping_ie && (self.rx_ping_ctrl & RX_CTRL_STATUS != 0))
            || (self.cfg.ping_pong && rx_pong_ie && (self.rx_pong_ctrl & RX_CTRL_STATUS != 0));

        tx_irq || rx_irq
    }

    fn receive_frame(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() {
            return false;
        }
        let len = frame.len().min(BUFFER_SIZE);

        // Try Ping buffer first
        if (self.rx_ping_ctrl & RX_CTRL_STATUS) == 0 {
            self.rx_ping_buf[..len].copy_from_slice(&frame[..len]);
            self.rx_ping_len = len as u32;
            self.rx_ping_ctrl |= RX_CTRL_STATUS;
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += len as u64;
            return true;
        }

        // If ping is busy and ping-pong enabled, try Pong buffer
        if self.cfg.ping_pong && (self.rx_pong_ctrl & RX_CTRL_STATUS) == 0 {
            self.rx_pong_buf[..len].copy_from_slice(&frame[..len]);
            self.rx_pong_len = len as u32;
            self.rx_pong_ctrl |= RX_CTRL_STATUS;
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += len as u64;
            return true;
        }

        // Buffer overrun: drop frame
        self.stats.rx_dropped += 1;
        false
    }

    fn transmit_ping(&mut self) {
        let len = (self.tx_ping_len & 0x7FF) as usize;
        let len = len.min(BUFFER_SIZE);
        if len > 0 {
            let frame = self.tx_ping_buf[..len].to_vec();
            self.stats.tx_packets += 1;
            self.stats.tx_bytes += len as u64;

            if self.cfg.loopback || (self.tx_ping_ctrl & TX_CTRL_LOOPBACK) != 0 {
                self.receive_frame(&frame);
            }
            self.from_tx.push_back(frame);
        }
        self.tx_ping_ctrl &= !TX_CTRL_BUSY;
        self.tx_ping_int = true;
    }

    fn transmit_pong(&mut self) {
        let len = (self.tx_pong_len & 0x7FF) as usize;
        let len = len.min(BUFFER_SIZE);
        if len > 0 {
            let frame = self.tx_pong_buf[..len].to_vec();
            self.stats.tx_packets += 1;
            self.stats.tx_bytes += len as u64;

            if self.cfg.loopback || (self.tx_pong_ctrl & TX_CTRL_LOOPBACK) != 0 {
                self.receive_frame(&frame);
            }
            self.from_tx.push_back(frame);
        }
        self.tx_pong_ctrl &= !TX_CTRL_BUSY;
        self.tx_pong_int = true;
    }

    fn read_reg(&mut self, offset: u64) -> u32 {
        match offset {
            TX_PING_LEN => self.tx_ping_len,
            MDIOADDR => self.mdio_addr,
            MDIOWR => self.mdio_wr,
            MDIORD => self.mdio_rd,
            MDIOCTRL => self.mdio_ctrl,
            GIE => {
                if self.gie {
                    GIE_ENABLE
                } else {
                    0
                }
            }
            TX_PING_CTRL => self.tx_ping_ctrl,

            TX_PONG_LEN if self.cfg.ping_pong => self.tx_pong_len,
            TX_PONG_CTRL if self.cfg.ping_pong => self.tx_pong_ctrl,

            RX_PING_LEN | RX_PING_LEN_ALT => self.rx_ping_len,
            RX_PING_CTRL => self.rx_ping_ctrl,

            RX_PONG_LEN | RX_PONG_LEN_ALT if self.cfg.ping_pong => self.rx_pong_len,
            RX_PONG_CTRL if self.cfg.ping_pong => self.rx_pong_ctrl,

            // Buffer accesses
            o if (TX_PING_BASE..TX_PING_BASE + BUFFER_SIZE as u64).contains(&o) => {
                read_buf_le(&self.tx_ping_buf, (o - TX_PING_BASE) as usize)
            }
            o if self.cfg.ping_pong
                && (TX_PONG_BASE..TX_PONG_BASE + BUFFER_SIZE as u64).contains(&o) =>
            {
                read_buf_le(&self.tx_pong_buf, (o - TX_PONG_BASE) as usize)
            }
            o if (RX_PING_BASE..RX_PING_BASE + BUFFER_SIZE as u64).contains(&o) => {
                read_buf_le(&self.rx_ping_buf, (o - RX_PING_BASE) as usize)
            }
            o if self.cfg.ping_pong
                && (RX_PONG_BASE..RX_PONG_BASE + BUFFER_SIZE as u64).contains(&o) =>
            {
                read_buf_le(&self.rx_pong_buf, (o - RX_PONG_BASE) as usize)
            }
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, strobe: u8) {
        match offset {
            TX_PING_LEN => self.tx_ping_len = value & 0x7FF,
            MDIOADDR => {
                self.mdio_addr = value;
                let reg_addr = (value & 0x1F) as usize;
                let is_read = (value & (1 << 10)) != 0;
                if is_read {
                    if reg_addr < self.phy_regs.len() {
                        self.mdio_rd = u32::from(self.phy_regs[reg_addr]);
                    }
                } else if reg_addr < self.phy_regs.len() {
                    self.phy_regs[reg_addr] = (self.mdio_wr & 0xFFFF) as u16;
                }
                self.mdio_ctrl |= MDIOCTRL_READY;
            }
            MDIOWR => {
                self.mdio_wr = value;
                self.mdio_ctrl |= MDIOCTRL_READY;
            }
            MDIOCTRL => self.mdio_ctrl = value | MDIOCTRL_READY,
            GIE => self.gie = (value & GIE_ENABLE) != 0,
            TX_PING_CTRL => {
                self.tx_ping_ctrl = value
                    & (TX_CTRL_BUSY
                        | TX_CTRL_PROGRAM
                        | TX_CTRL_IE
                        | TX_CTRL_LOOPBACK
                        | 0x8000_0000);
                self.tx_ping_int = false;
                if (value & TX_CTRL_BUSY) != 0 {
                    self.transmit_ping();
                }
            }

            TX_PONG_LEN if self.cfg.ping_pong => self.tx_pong_len = value & 0x7FF,
            TX_PONG_CTRL if self.cfg.ping_pong => {
                self.tx_pong_ctrl = value
                    & (TX_CTRL_BUSY
                        | TX_CTRL_PROGRAM
                        | TX_CTRL_IE
                        | TX_CTRL_LOOPBACK
                        | 0x8000_0000);
                self.tx_pong_int = false;
                if (value & TX_CTRL_BUSY) != 0 {
                    self.transmit_pong();
                }
            }

            RX_PING_LEN | RX_PING_LEN_ALT => self.rx_ping_len = value & 0x7FF,
            RX_PING_CTRL => {
                // Writing 0 to bit 0 clears RX_STATUS (releases buffer to hardware)
                self.rx_ping_ctrl = value & (RX_CTRL_STATUS | RX_CTRL_IE);
            }

            RX_PONG_LEN | RX_PONG_LEN_ALT if self.cfg.ping_pong => self.rx_pong_len = value & 0x7FF,
            RX_PONG_CTRL if self.cfg.ping_pong => {
                self.rx_pong_ctrl = value & (RX_CTRL_STATUS | RX_CTRL_IE);
            }

            // Buffer accesses
            o if (TX_PING_BASE..TX_PING_BASE + BUFFER_SIZE as u64).contains(&o) => {
                write_buf_le(
                    &mut self.tx_ping_buf,
                    (o - TX_PING_BASE) as usize,
                    value,
                    strobe,
                );
            }
            o if self.cfg.ping_pong
                && (TX_PONG_BASE..TX_PONG_BASE + BUFFER_SIZE as u64).contains(&o) =>
            {
                write_buf_le(
                    &mut self.tx_pong_buf,
                    (o - TX_PONG_BASE) as usize,
                    value,
                    strobe,
                );
            }
            o if (RX_PING_BASE..RX_PING_BASE + BUFFER_SIZE as u64).contains(&o) => {
                write_buf_le(
                    &mut self.rx_ping_buf,
                    (o - RX_PING_BASE) as usize,
                    value,
                    strobe,
                );
            }
            o if self.cfg.ping_pong
                && (RX_PONG_BASE..RX_PONG_BASE + BUFFER_SIZE as u64).contains(&o) =>
            {
                write_buf_le(
                    &mut self.rx_pong_buf,
                    (o - RX_PONG_BASE) as usize,
                    value,
                    strobe,
                );
            }
            _ => {}
        }
    }
}

fn read_buf_le(buf: &[u8], offset: usize) -> u32 {
    let mut b = [0u8; 4];
    for (i, slot) in b.iter_mut().enumerate() {
        if offset + i < buf.len() {
            *slot = buf[offset + i];
        }
    }
    u32::from_le_bytes(b)
}

fn write_buf_le(buf: &mut [u8], offset: usize, value: u32, strobe: u8) {
    let bytes = value.to_le_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if (strobe & (1 << i)) != 0 && offset + i < buf.len() {
            buf[offset + i] = *b;
        }
    }
}

/// Behavioural register-level model of the Xilinx AXI Ethernet Lite core.
pub struct AxiEthernetLite {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Handle to inspect or control an AXI Ethernet Lite instance outside register accesses.
#[derive(Clone)]
pub struct EthernetLiteHandle {
    st: Arc<Mutex<State>>,
}

impl AxiEthernetLite {
    /// Creates a new AXI Ethernet Lite instance and returns `(AxiEthernetLite, EthernetLiteHandle)`.
    pub fn new(name: &str, cfg: EthernetLiteConfig) -> (AxiEthernetLite, EthernetLiteHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxiEthernetLite {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            EthernetLiteHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EthernetLiteHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Configuration of the instance.
    pub fn config(&self) -> EthernetLiteConfig {
        self.lock().cfg.clone()
    }

    /// Drains and returns all Ethernet frames transmitted by the core.
    pub fn take_tx(&self) -> Vec<Vec<u8>> {
        self.lock().from_tx.drain(..).collect()
    }

    /// Injects an Ethernet frame into the receiver. Returns `true` if accepted, `false` if dropped.
    pub fn push_rx(&self, frame: &[u8]) -> bool {
        self.lock().receive_frame(frame)
    }

    /// Performance and packet counters.
    pub fn stats(&self) -> EthernetLiteStats {
        self.lock().stats
    }
}

impl RegisterBlock for AxiEthernetLite {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        self.lock().read_reg(offset)
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        self.lock().write_reg(offset, value, strobe);
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        let mut s = self.lock();
        s.now = s.now.max(now_ns);
    }

    fn irq_level(&self) -> bool {
        self.irq_output(0)
    }

    fn irq_output(&self, output: usize) -> bool {
        if output == 0 {
            self.lock().irq()
        } else {
            false
        }
    }

    fn service(&mut self, now_ns: u64, _ps: &mut dyn PsPort) {
        let mut s = self.lock();
        s.now = s.now.max(now_ns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_mdio_read() {
        let (mut eth, _) = AxiEthernetLite::new("eth0", EthernetLiteConfig::default());
        assert_eq!(eth.size(), 0x2000);
        assert!(!eth.irq_level());

        // Read PHY basic status register (reg 1) via MDIO
        // OP=1 (bit 10), PHY=1 (bits 9:5), REG=1 (bits 4:0)
        let cmd = (1 << 10) | (1 << 5) | 1;
        eth.write32(MDIOADDR, cmd, 0xF);
        let rd = eth.read32(MDIORD);
        assert_eq!(rd, 0x786D, "Basic status reg indicates link up");

        // Read PHY ID 1 (reg 2)
        let cmd2 = (1 << 10) | (1 << 5) | 2;
        eth.write32(MDIOADDR, cmd2, 0xF);
        assert_eq!(eth.read32(MDIORD), 0x0141);
    }

    #[test]
    fn loopback_packet_transfer_and_interrupt() {
        let (mut eth, handle) = AxiEthernetLite::new("eth0", EthernetLiteConfig::default());

        // Global interrupt enable
        eth.write32(GIE, GIE_ENABLE, 0xF);

        // Enable TX and RX interrupts
        eth.write32(TX_PING_CTRL, TX_CTRL_IE, 0xF);
        eth.write32(RX_PING_CTRL, RX_CTRL_IE, 0xF);
        assert!(!eth.irq_level());

        // Fill TX buffer with 64-byte frame
        let mut frame = [0u8; 64];
        for (i, b) in frame.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(0x10);
        }
        for (i, chunk) in frame.chunks(4).enumerate() {
            let mut w = [0u8; 4];
            w.copy_from_slice(chunk);
            eth.write32(TX_PING_BASE + (i * 4) as u64, u32::from_le_bytes(w), 0xF);
        }

        // Set TX frame length
        eth.write32(TX_PING_LEN, 64, 0xF);

        // Start transmit
        eth.write32(TX_PING_CTRL, TX_CTRL_BUSY | TX_CTRL_IE, 0xF);

        // Loopback should deliver frame to RX buffer
        assert!(eth.irq_level(), "Interrupt must be asserted");
        let rx_ctrl = eth.read32(RX_PING_CTRL);
        assert_ne!(rx_ctrl & RX_CTRL_STATUS, 0, "RX frame ready");

        let rx_len = eth.read32(RX_PING_LEN);
        assert_eq!(rx_len, 64);

        // Read back frame from RX buffer and verify
        let mut rx_frame = [0u8; 64];
        for (i, chunk) in rx_frame.chunks_mut(4).enumerate() {
            let w = eth.read32(RX_PING_BASE + (i * 4) as u64);
            chunk.copy_from_slice(&w.to_le_bytes());
        }
        assert_eq!(rx_frame, frame);

        let tx_packets = handle.take_tx();
        assert_eq!(tx_packets.len(), 1);
        assert_eq!(tx_packets[0], frame.to_vec());

        // Acknowledging TX and releasing RX buffer lowers interrupt
        eth.write32(TX_PING_CTRL, TX_CTRL_IE, 0xF);
        eth.write32(RX_PING_CTRL, RX_CTRL_IE, 0xF);
        assert!(!eth.irq_level(), "Interrupt must deassert when cleared");
    }

    #[test]
    fn host_push_rx_packet() {
        let (mut eth, handle) = AxiEthernetLite::new("eth0", EthernetLiteConfig::default());
        eth.write32(GIE, GIE_ENABLE, 0xF);
        eth.write32(RX_PING_CTRL, RX_CTRL_IE, 0xF);

        let external_pkt = vec![0xEE; 48];
        let accepted = handle.push_rx(&external_pkt);
        assert!(accepted);

        assert!(eth.irq_level());
        assert_eq!(eth.read32(RX_PING_LEN), 48);
        assert_eq!(eth.read32(RX_PING_BASE), 0xEEEE_EEEE);
    }
}
