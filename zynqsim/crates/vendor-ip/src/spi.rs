//! AXI Quad SPI (`xilinx.com:ip:axi_quad_spi:3.2`, Xilinx PG153), register level.
//!
//! Register map (offsets from the base address; from PG153 Table 2-4):
//!
//! | offset | register | description | access | reset |
//! |---|---|---|---|---|
//! | `0x1C` | `DGIER` | Device Global Interrupt Enable | R/W | `0x0000_0000` |
//! | `0x20` | `IPISR` | IP Interrupt Status Register | R/TOW | `0x0000_0000` |
//! | `0x28` | `IPIER` | IP Interrupt Enable Register | R/W | `0x0000_0000` |
//! | `0x40` | `SRR` | Software Reset Register (write `0xA`) | W | `0x0000_0000` |
//! | `0x60` | `SPICR` | SPI Control Register | R/W | `0x0000_0180` |
//! | `0x64` | `SPISR` | SPI Status Register | R | `0x0000_0005` (master) / `0x25` |
//! | `0x68` | `SPIDTR` | SPI Data Transmit Register | W | `0x0000_0000` |
//! | `0x6C` | `SPIDRR` | SPI Data Receive Register | R | `0x0000_0000` |
//! | `0x70` | `SPISSR` | SPI Slave Select Register (active low) | R/W | `0xFFFF_FFFF` |
//! | `0x74` | `TX_FIFO_OCY` | Transmit FIFO Occupancy | R | `0x0000_0000` |
//! | `0x78` | `RX_FIFO_OCY` | Receive FIFO Occupancy | R | `0x0000_0000` |
//!
//! `SPICR`:
//! - bit 0 `LOOP`: loopback mode enable.
//! - bit 1 `SPE`: SPI system enable (1 = enabled).
//! - bit 2 `MASTER`: 1 = master mode, 0 = slave mode.
//! - bit 3 `CPOL`: clock polarity.
//! - bit 4 `CPHA`: clock phase.
//! - bit 5 `TX_FIFO_RESET`: write 1 resets TX FIFO (self-clearing).
//! - bit 6 `RX_FIFO_RESET`: write 1 resets RX FIFO (self-clearing).
//! - bit 7 `MANUAL_SS`: manual slave select assertion (1 = SPISSR controls SS; 0 = auto).
//! - bit 8 `MASTER_INHIBIT`: master transaction inhibit (1 = disabled, 0 = enabled).
//! - bit 9 `LSB_FIRST`: 1 = LSB first, 0 = MSB first.
//!
//! `SPISR`:
//! - bit 0 `RX_EMPTY`: 1 if receive FIFO is empty.
//! - bit 1 `RX_FULL`: 1 if receive FIFO is full.
//! - bit 2 `TX_EMPTY`: 1 if transmit FIFO is empty.
//! - bit 3 `TX_FULL`: 1 if transmit FIFO is full.
//! - bit 4 `MODF`: mode fault error.
//! - bit 5 `SLAVE_MODE_SELECT`: 1 if in slave mode.
//!
//! `IPISR` / `IPIER`:
//! - bit 0 `MODF`: mode fault error.
//! - bit 1 `SLAVE_MODF`: slave mode fault error.
//! - bit 2 `DTR_EMPTY`: transmit FIFO empty.
//! - bit 3 `DTR_UNDERRUN`: transmit underrun.
//! - bit 4 `DRR_FULL`: receive FIFO full.
//! - bit 5 `DRR_OVERRUN`: receive overrun.
//! - bit 6 `TX_HALF_EMPTY`: transmit FIFO half empty.
//! - bit 7 `SLAVE_MODE`: slave mode select.
//! - bit 8 `DRR_NOT_EMPTY`: receive FIFO not empty.
//!
//! `VERIFY:` items:
//! * Self-test / loopback behavior: when `LOOP == 1`, `SPE == 1`, and `MASTER_INHIBIT == 0`,
//!   bytes written to SPIDTR are shifted directly into SPIDRR, setting `DRR_NOT_EMPTY` and
//!   `DTR_EMPTY`.
//! * FIFO occupancy registers report 0 when empty, and `len - 1` when holding entries.
//! * Unmapped offsets return 0 and ignore writes.

use crate::params::{Params, merge, ranged};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `DGIER`: Device Global Interrupt Enable Register.
pub const DGIER: u64 = 0x1C;
/// `IPISR`: IP Interrupt Status Register.
pub const IPISR: u64 = 0x20;
/// `IPIER`: IP Interrupt Enable Register.
pub const IPIER: u64 = 0x28;
/// `SRR`: Software Reset Register.
pub const SRR: u64 = 0x40;
/// `SPICR`: SPI Control Register.
pub const SPICR: u64 = 0x60;
/// `SPISR`: SPI Status Register.
pub const SPISR: u64 = 0x64;
/// `SPIDTR`: SPI Data Transmit Register.
pub const SPIDTR: u64 = 0x68;
/// `SPIDRR`: SPI Data Receive Register.
pub const SPIDRR: u64 = 0x6C;
/// `SPISSR`: SPI Slave Select Register.
pub const SPISSR: u64 = 0x70;
/// `TX_FIFO_OCY`: Transmit FIFO Occupancy Register.
pub const TX_FIFO_OCY: u64 = 0x74;
/// `RX_FIFO_OCY`: Receive FIFO Occupancy Register.
pub const RX_FIFO_OCY: u64 = 0x78;

/// DGIER global interrupt enable mask (bit 31).
pub const DGIER_GIE: u32 = 1 << 31;

/// SPICR bit 0: Loopback mode.
pub const SPICR_LOOP: u32 = 1 << 0;
/// SPICR bit 1: SPI System Enable.
pub const SPICR_SPE: u32 = 1 << 1;
/// SPICR bit 2: Master mode.
pub const SPICR_MASTER: u32 = 1 << 2;
/// SPICR bit 3: Clock polarity.
pub const SPICR_CPOL: u32 = 1 << 3;
/// SPICR bit 4: Clock phase.
pub const SPICR_CPHA: u32 = 1 << 4;
/// SPICR bit 5: TX FIFO reset.
pub const SPICR_TX_FIFO_RESET: u32 = 1 << 5;
/// SPICR bit 6: RX FIFO reset.
pub const SPICR_RX_FIFO_RESET: u32 = 1 << 6;
/// SPICR bit 7: Manual Slave Select assertion.
pub const SPICR_MANUAL_SS: u32 = 1 << 7;
/// SPICR bit 8: Master transaction inhibit.
pub const SPICR_MASTER_INHIBIT: u32 = 1 << 8;
/// SPICR bit 9: LSB first.
pub const SPICR_LSB_FIRST: u32 = 1 << 9;

/// SPISR bit 0: RX FIFO empty.
pub const SPISR_RX_EMPTY: u32 = 1 << 0;
/// SPISR bit 1: RX FIFO full.
pub const SPISR_RX_FULL: u32 = 1 << 1;
/// SPISR bit 2: TX FIFO empty.
pub const SPISR_TX_EMPTY: u32 = 1 << 2;
/// SPISR bit 3: TX FIFO full.
pub const SPISR_TX_FULL: u32 = 1 << 3;
/// SPISR bit 4: Mode fault error.
pub const SPISR_MODF: u32 = 1 << 4;
/// SPISR bit 5: Slave mode select.
pub const SPISR_SLAVE_MODE: u32 = 1 << 5;

/// IPISR/IPIER bit 0: Mode fault error.
pub const INTR_MODF: u32 = 1 << 0;
/// IPISR/IPIER bit 1: Slave mode fault error.
pub const INTR_SLAVE_MODF: u32 = 1 << 1;
/// IPISR/IPIER bit 2: DTR (TX FIFO) empty.
pub const INTR_DTR_EMPTY: u32 = 1 << 2;
/// IPISR/IPIER bit 3: DTR underrun.
pub const INTR_DTR_UNDERRUN: u32 = 1 << 3;
/// IPISR/IPIER bit 4: DRR (RX FIFO) full.
pub const INTR_DRR_FULL: u32 = 1 << 4;
/// IPISR/IPIER bit 5: DRR overrun.
pub const INTR_DRR_OVERRUN: u32 = 1 << 5;
/// IPISR/IPIER bit 6: TX FIFO half empty.
pub const INTR_TX_HALF_EMPTY: u32 = 1 << 6;
/// IPISR/IPIER bit 7: Slave mode select.
pub const INTR_SLAVE_MODE: u32 = 1 << 7;
/// IPISR/IPIER bit 8: DRR (RX FIFO) not empty.
pub const INTR_DRR_NOT_EMPTY: u32 = 1 << 8;

/// SRR software reset key (PG153).
pub const SRR_RESET_KEY: u32 = 0x0000_000A;

/// IP configuration parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpiConfig {
    /// Depth of the transmit and receive FIFOs (16 or 256; default 16).
    pub fifo_depth: usize,
    /// Number of slave select bits (1..=32; default 1).
    pub num_ss_bits: u32,
    /// Clock frequency of ACLK in Hz (default 100 MHz).
    pub clk_freq_hz: u64,
    /// Address window size in bytes (default 64 KiB).
    pub window: u64,
}

impl Default for SpiConfig {
    fn default() -> Self {
        SpiConfig {
            fifo_depth: 16,
            num_ss_bits: 1,
            clk_freq_hz: 100_000_000,
            window: 0x1_0000,
        }
    }
}

impl SpiConfig {
    /// Builds configuration from parameter dictionary.
    pub fn from_params(p: &Params, window: u64) -> Result<SpiConfig, String> {
        let depth = ranged(p, "C_FIFO_DEPTH", 16, 0, 256)? as usize;
        let fifo_depth = if depth >= 256 {
            256
        } else if depth == 0 {
            1
        } else {
            16
        };
        let num_ss_bits = ranged(p, "C_NUM_SS_BITS", 1, 1, 32)? as u32;
        let clk_freq_hz = ranged(p, "CLK_FREQ_HZ", 100_000_000, 1_000, 1_000_000_000)?;
        Ok(SpiConfig {
            fifo_depth,
            num_ss_bits,
            clk_freq_hz,
            window,
        })
    }
}

struct State {
    cfg: SpiConfig,
    dgier: u32,
    ipisr: u32,
    ipier: u32,
    spicr: u32,
    spissr: u32,
    tx_fifo: VecDeque<u8>,
    rx_fifo: VecDeque<u8>,
    /// Historical log of transferred bytes (MOSI, MISO) with virtual timestamps.
    transfers: Vec<(u64, u8, u8)>,
}

impl State {
    fn new(cfg: SpiConfig) -> State {
        let mut s = State {
            cfg,
            dgier: 0,
            ipisr: 0,
            ipier: 0,
            spicr: SPICR_MANUAL_SS | SPICR_MASTER_INHIBIT, // 0x0180
            spissr: 0xFFFF_FFFF,
            tx_fifo: VecDeque::new(),
            rx_fifo: VecDeque::new(),
            transfers: Vec::new(),
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.dgier = 0;
        self.ipisr = 0;
        self.ipier = 0;
        self.spicr = SPICR_MANUAL_SS | SPICR_MASTER_INHIBIT; // 0x0180
        self.spissr = 0xFFFF_FFFF;
        self.tx_fifo.clear();
        self.rx_fifo.clear();
    }

    fn spisr(&self) -> u32 {
        let mut sr = 0;
        if self.rx_fifo.is_empty() {
            sr |= SPISR_RX_EMPTY;
        } else if self.rx_fifo.len() >= self.cfg.fifo_depth {
            sr |= SPISR_RX_FULL;
        }
        if self.tx_fifo.is_empty() {
            sr |= SPISR_TX_EMPTY;
        } else if self.tx_fifo.len() >= self.cfg.fifo_depth {
            sr |= SPISR_TX_FULL;
        }
        if self.spicr & SPICR_MASTER == 0 {
            sr |= SPISR_SLAVE_MODE;
        }
        sr
    }

    fn tx_occupancy(&self) -> u32 {
        if self.tx_fifo.is_empty() {
            0
        } else {
            (self.tx_fifo.len() - 1) as u32
        }
    }

    fn rx_occupancy(&self) -> u32 {
        if self.rx_fifo.is_empty() {
            0
        } else {
            (self.rx_fifo.len() - 1) as u32
        }
    }

    fn irq(&self) -> bool {
        let gie = (self.dgier & DGIER_GIE) != 0;
        let active = (self.ipisr & self.ipier) != 0;
        gie && active
    }

    /// Performs SPI transfers if core is enabled (`SPE = 1`) and not inhibited (`MASTER_INHIBIT = 0`).
    fn pump(&mut self, now_ns: u64) {
        let spe = (self.spicr & SPICR_SPE) != 0;
        let inhibit = (self.spicr & SPICR_MASTER_INHIBIT) != 0;
        if !spe || inhibit {
            return;
        }

        let loopback = (self.spicr & SPICR_LOOP) != 0;
        let had_tx = !self.tx_fifo.is_empty();

        while let Some(tx_byte) = self.tx_fifo.pop_front() {
            let rx_byte = if loopback {
                tx_byte
            } else {
                // If not loopback, floating MISO pulls high (0xFF)
                0xFF
            };

            if self.rx_fifo.len() < self.cfg.fifo_depth {
                self.rx_fifo.push_back(rx_byte);
                self.ipisr |= INTR_DRR_NOT_EMPTY;
                if self.rx_fifo.len() >= self.cfg.fifo_depth {
                    self.ipisr |= INTR_DRR_FULL;
                }
            } else {
                self.ipisr |= INTR_DRR_OVERRUN;
            }

            self.transfers.push((now_ns, tx_byte, rx_byte));
        }

        if had_tx && self.tx_fifo.is_empty() {
            self.ipisr |= INTR_DTR_EMPTY;
        }
    }
}

/// AXI Quad SPI model as a [`RegisterBlock`].
pub struct AxiQuadSpi {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Clonable handle for host / test inspection.
#[derive(Clone)]
pub struct SpiHandle {
    st: Arc<Mutex<State>>,
}

impl AxiQuadSpi {
    /// Creates a new AXI Quad SPI instance.
    pub fn new(name: &str, cfg: SpiConfig) -> (AxiQuadSpi, SpiHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxiQuadSpi {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            SpiHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SpiHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Total transfers performed since start.
    pub fn transfer_count(&self) -> usize {
        self.lock().transfers.len()
    }

    /// Transferred bytes: `(time_ns, mosi, miso)`.
    pub fn transfers(&self) -> Vec<(u64, u8, u8)> {
        self.lock().transfers.clone()
    }

    /// Injects a byte into the receive FIFO directly (host injection).
    pub fn inject_rx(&self, byte: u8) -> bool {
        let mut s = self.lock();
        if s.rx_fifo.len() < s.cfg.fifo_depth {
            s.rx_fifo.push_back(byte);
            s.ipisr |= INTR_DRR_NOT_EMPTY;
            true
        } else {
            false
        }
    }
}

impl RegisterBlock for AxiQuadSpi {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let mut s = self.lock();
        match offset {
            DGIER => s.dgier,
            IPISR => s.ipisr,
            IPIER => s.ipier,
            SRR => 0,
            SPICR => s.spicr,
            SPISR => s.spisr(),
            SPIDTR => 0, // Write-only
            SPIDRR => {
                let val = s.rx_fifo.pop_front().unwrap_or(0);
                u32::from(val)
            }
            SPISSR => s.spissr,
            TX_FIFO_OCY => s.tx_occupancy(),
            RX_FIFO_OCY => s.rx_occupancy(),
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        match offset {
            DGIER => {
                let cur = s.dgier;
                s.dgier = merge(cur, value, strobe) & DGIER_GIE;
            }
            IPISR => {
                // Toggle on write / Write-1-to-Clear
                let mask = merge(0, value, strobe);
                s.ipisr &= !mask;
            }
            IPIER => {
                let cur = s.ipier;
                s.ipier = merge(cur, value, strobe) & 0x3FFF;
            }
            SRR => {
                if merge(0, value, strobe) == SRR_RESET_KEY {
                    s.reset();
                }
            }
            SPICR => {
                let cur = s.spicr;
                let new_cr = merge(cur, value, strobe);

                if new_cr & SPICR_TX_FIFO_RESET != 0 {
                    s.tx_fifo.clear();
                }
                if new_cr & SPICR_RX_FIFO_RESET != 0 {
                    s.rx_fifo.clear();
                }

                // Bits 5 and 6 are self-clearing
                s.spicr = new_cr & !(SPICR_TX_FIFO_RESET | SPICR_RX_FIFO_RESET);
                s.pump(0);
            }
            SPIDTR => {
                let byte = (merge(0, value, strobe) & 0xFF) as u8;
                if s.tx_fifo.len() < s.cfg.fifo_depth {
                    s.tx_fifo.push_back(byte);
                } else {
                    s.ipisr |= INTR_DTR_UNDERRUN;
                }
                s.pump(0);
            }
            SPISSR => {
                let cur = s.spissr;
                let mask = (1u64 << s.cfg.num_ss_bits) - 1;
                let written = merge(cur, value, strobe);
                s.spissr = (written & mask as u32) | !(mask as u32);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        self.lock().pump(now_ns);
    }

    fn irq_level(&self) -> bool {
        self.lock().irq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state_and_registers() {
        let (mut spi, _) = AxiQuadSpi::new("spi0", SpiConfig::default());
        assert_eq!(spi.read32(DGIER), 0);
        assert_eq!(spi.read32(IPISR), 0);
        assert_eq!(spi.read32(IPIER), 0);
        assert_eq!(spi.read32(SPICR), 0x0180); // MANUAL_SS | MASTER_INHIBIT
        assert_eq!(
            spi.read32(SPISR),
            SPISR_RX_EMPTY | SPISR_TX_EMPTY | SPISR_SLAVE_MODE
        );
        assert_eq!(spi.read32(SPISSR), 0xFFFF_FFFF);
        assert_eq!(spi.read32(TX_FIFO_OCY), 0);
        assert_eq!(spi.read32(RX_FIFO_OCY), 0);
        assert!(!spi.irq_level());
    }

    #[test]
    fn software_reset_via_srr() {
        let (mut spi, _) = AxiQuadSpi::new("spi0", SpiConfig::default());
        spi.write32(DGIER, DGIER_GIE, 0xF);
        spi.write32(IPIER, 0x3FFF, 0xF);
        spi.write32(SPICR, SPICR_SPE | SPICR_MASTER, 0xF);
        assert_eq!(spi.read32(DGIER), DGIER_GIE);

        // Writing non-reset value does nothing
        spi.write32(SRR, 0x1234, 0xF);
        assert_eq!(spi.read32(DGIER), DGIER_GIE);

        // Writing 0xA resets all
        spi.write32(SRR, 0xA, 0xF);
        assert_eq!(spi.read32(DGIER), 0);
        assert_eq!(spi.read32(IPIER), 0);
        assert_eq!(spi.read32(SPICR), 0x0180);
    }

    #[test]
    fn loopback_mode_transfers_bytes_and_interrupts() {
        let (mut spi, h) = AxiQuadSpi::new("spi0", SpiConfig::default());

        // Enable DGIER and DTR_EMPTY + DRR_NOT_EMPTY interrupts
        spi.write32(DGIER, DGIER_GIE, 0xF);
        spi.write32(IPIER, INTR_DTR_EMPTY | INTR_DRR_NOT_EMPTY, 0xF);

        // Configure SPICR: LOOP=1, SPE=1, MASTER=1, MASTER_INHIBIT=0
        spi.write32(SPICR, SPICR_LOOP | SPICR_SPE | SPICR_MASTER, 0xF);
        assert_eq!(spi.read32(SPISR) & SPISR_SLAVE_MODE, 0); // Master mode

        // Write a byte to SPIDTR
        spi.write32(SPIDTR, 0x5A, 0xF);

        // In loopback, the byte transfers into DRR immediately
        assert_eq!(spi.read32(SPISR) & SPISR_TX_EMPTY, SPISR_TX_EMPTY);
        assert_eq!(spi.read32(SPISR) & SPISR_RX_EMPTY, 0);
        assert_eq!(spi.read32(RX_FIFO_OCY), 0); // 1 item -> len - 1 = 0
        assert_eq!(h.transfer_count(), 1);

        // Interrupt should be active
        assert!(spi.irq_level());
        let isr = spi.read32(IPISR);
        assert_ne!(isr & INTR_DTR_EMPTY, 0);
        assert_ne!(isr & INTR_DRR_NOT_EMPTY, 0);

        // Read the byte back from DRR
        let val = spi.read32(SPIDRR);
        assert_eq!(val, 0x5A);

        // Now RX is empty again
        assert_eq!(spi.read32(SPISR) & SPISR_RX_EMPTY, SPISR_RX_EMPTY);

        // Clear IPISR (write 1 to clear)
        spi.write32(IPISR, isr, 0xF);
        assert_eq!(spi.read32(IPISR), 0);
        assert!(!spi.irq_level());
    }

    #[test]
    fn fifo_resets_via_spicr() {
        let (mut spi, _) = AxiQuadSpi::new("spi0", SpiConfig::default());
        // Inhibit transfers so bytes stay in TX FIFO
        spi.write32(SPICR, SPICR_MASTER_INHIBIT, 0xF);
        spi.write32(SPIDTR, 0x11, 0xF);
        spi.write32(SPIDTR, 0x22, 0xF);
        assert_eq!(spi.read32(TX_FIFO_OCY), 1); // 2 items -> 1

        // Reset TX FIFO via SPICR bit 5
        spi.write32(SPICR, SPICR_MASTER_INHIBIT | SPICR_TX_FIFO_RESET, 0xF);
        assert_eq!(spi.read32(TX_FIFO_OCY), 0);
        assert_eq!(spi.read32(SPISR) & SPISR_TX_EMPTY, SPISR_TX_EMPTY);
        // Bit 5 is self-clearing
        assert_eq!(spi.read32(SPICR) & SPICR_TX_FIFO_RESET, 0);
    }

    #[test]
    fn slave_select_register_masks_num_ss_bits() {
        let cfg = SpiConfig {
            num_ss_bits: 2,
            ..SpiConfig::default()
        };
        let (mut spi, _) = AxiQuadSpi::new("spi0", cfg);
        assert_eq!(spi.read32(SPISSR), 0xFFFF_FFFF);

        // Assert slave 0 (bit 0 = 0)
        spi.write32(SPISSR, 0xFFFF_FFFE, 0xF);
        assert_eq!(spi.read32(SPISSR), 0xFFFF_FFFE);

        // Assert slave 1 (bit 1 = 0)
        spi.write32(SPISSR, 0xFFFF_FFFD, 0xF);
        assert_eq!(spi.read32(SPISSR), 0xFFFF_FFFD);

        // Bit 2 is beyond num_ss_bits (2), so it is masked to 1
        spi.write32(SPISSR, 0xFFFF_FFF8, 0xF);
        assert_eq!(spi.read32(SPISSR), 0xFFFF_FFFC);
    }
}
