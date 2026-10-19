//! AXI UART 16550 (`xilinx.com:ip:axi_uart16550:2.0`, Xilinx PG143), register level.
//!
//! Standard National Semiconductor 16550A compatible UART mapped into an AXI4-Lite slave
//! with a 64 KiB address window (`0x10000` bytes). The 16550 register set is mapped at base
//! offset `0x1000` (`XUN_REG_OFFSET` in Xilinx Vitis `xuartns550_l.h`).
//!
//! Register map (PG143):
//!
//! | Offset | DLAB | Register | Access | Description |
//! |---|---|---|---|---|
//! | `0x1000` | 0 | `RBR` | R | Receiver Buffer Register |
//! | `0x1000` | 0 | `THR` | W | Transmitter Holding Register |
//! | `0x1000` | 1 | `DLL` | R/W | Divisor Latch (LSB) |
//! | `0x1004` | 0 | `IER` | R/W | Interrupt Enable Register |
//! | `0x1004` | 1 | `DLM` | R/W | Divisor Latch (MSB) |
//! | `0x1008` | X | `IIR` | R | Interrupt Identification Register |
//! | `0x1008` | X | `FCR` | W | FIFO Control Register |
//! | `0x100C` | X | `LCR` | R/W | Line Control Register |
//! | `0x1010` | X | `MCR` | R/W | Modem Control Register |
//! | `0x1014` | X | `LSR` | R | Line Status Register |
//! | `0x1018` | X | `MSR` | R/W | Modem Status Register |
//! | `0x101C` | X | `SCR` | R/W | Scratchpad Register |

use crate::params::{Params, flag, number};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Base offset of the 16550 register bank inside the AXI window (XUN_REG_OFFSET).
pub const REG_OFFSET: u64 = 0x1000;

/// Offset of Receiver Buffer Register (read-only, DLAB = 0).
pub const RBR_OFFSET: u64 = 0x1000;
/// Offset of Transmitter Holding Register (write-only, DLAB = 0).
pub const THR_OFFSET: u64 = 0x1000;
/// Offset of Divisor Latch LSB (read/write, DLAB = 1).
pub const DLL_OFFSET: u64 = 0x1000;
/// Offset of Interrupt Enable Register (read/write, DLAB = 0).
pub const IER_OFFSET: u64 = 0x1004;
/// Offset of Divisor Latch MSB (read/write, DLAB = 1).
pub const DLM_OFFSET: u64 = 0x1004;
/// Offset of Interrupt Identification Register (read-only).
pub const IIR_OFFSET: u64 = 0x1008;
/// Offset of FIFO Control Register (write-only).
pub const FCR_OFFSET: u64 = 0x1008;
/// Offset of Line Control Register (read/write).
pub const LCR_OFFSET: u64 = 0x100C;
/// Offset of Modem Control Register (read/write).
pub const MCR_OFFSET: u64 = 0x1010;
/// Offset of Line Status Register (read-only).
pub const LSR_OFFSET: u64 = 0x1014;
/// Offset of Modem Status Register (read-only).
pub const MSR_OFFSET: u64 = 0x1018;
/// Offset of Scratchpad Register (read/write).
pub const SCR_OFFSET: u64 = 0x101C;

/// IER bit 0: Enable Received Data Available Interrupt.
pub const IER_ERBFI: u8 = 1 << 0;
/// IER bit 1: Enable Transmitter Holding Register Empty Interrupt.
pub const IER_ETBEI: u8 = 1 << 1;
/// IER bit 2: Enable Receiver Line Status Interrupt.
pub const IER_ELSI: u8 = 1 << 2;
/// IER bit 3: Enable Modem Status Interrupt.
pub const IER_EDSSI: u8 = 1 << 3;

/// IIR bit 0: No interrupt pending (active low).
pub const IIR_NO_INT: u8 = 1 << 0;
/// IIR bits 3:1 mask.
pub const IIR_ID_MASK: u8 = 0x0E;
/// IIR: Modem status interrupt.
pub const IIR_MODEM_STATUS: u8 = 0x00;
/// IIR: Transmitter holding register empty interrupt.
pub const IIR_THRE: u8 = 0x02;
/// IIR: Received data available interrupt.
pub const IIR_RX_DATA: u8 = 0x04;
/// IIR: Receiver line status interrupt.
pub const IIR_LINE_STATUS: u8 = 0x06;
/// IIR: Character timeout indication interrupt.
pub const IIR_TIMEOUT: u8 = 0x0C;
/// IIR: Bits 7:6 set when FIFOs enabled.
pub const IIR_FIFO_EN: u8 = 0xC0;

/// FCR bit 0: Enable RX/TX FIFOs.
pub const FCR_FIFO_EN: u8 = 1 << 0;
/// FCR bit 1: Reset RX FIFO.
pub const FCR_RX_RST: u8 = 1 << 1;
/// FCR bit 2: Reset TX FIFO.
pub const FCR_TX_RST: u8 = 1 << 2;
/// FCR bit 3: DMA mode select.
pub const FCR_DMA_MODE: u8 = 1 << 3;
/// FCR bits 7:6: RX trigger level mask.
pub const FCR_TRIG_MASK: u8 = 0xC0;

/// LCR bits 1:0: Word length select mask (00=5, 01=6, 10=7, 11=8).
pub const LCR_WLS_MASK: u8 = 0x03;
/// LCR bit 2: Number of stop bits (0=1, 1=2).
pub const LCR_STB: u8 = 1 << 2;
/// LCR bit 3: Parity enable.
pub const LCR_PEN: u8 = 1 << 3;
/// LCR bit 4: Even parity select.
pub const LCR_EPS: u8 = 1 << 4;
/// LCR bit 5: Stick parity.
pub const LCR_STICK: u8 = 1 << 5;
/// LCR bit 6: Break control.
pub const LCR_BREAK: u8 = 1 << 6;
/// LCR bit 7: Divisor Latch Access Bit (DLAB).
pub const LCR_DLAB: u8 = 1 << 7;

/// MCR bit 0: Data Terminal Ready (DTR).
pub const MCR_DTR: u8 = 1 << 0;
/// MCR bit 1: Request to Send (RTS).
pub const MCR_RTS: u8 = 1 << 1;
/// MCR bit 2: Auxiliary output 1 (OUT1).
pub const MCR_OUT1: u8 = 1 << 2;
/// MCR bit 3: Auxiliary output 2 (OUT2) / Interrupt output enable.
pub const MCR_OUT2: u8 = 1 << 3;
/// MCR bit 4: Diagnostic loopback mode.
pub const MCR_LOOP: u8 = 1 << 4;

/// LSR bit 0: Data Ready (DR).
pub const LSR_DR: u8 = 1 << 0;
/// LSR bit 1: Overrun Error (OE).
pub const LSR_OE: u8 = 1 << 1;
/// LSR bit 2: Parity Error (PE).
pub const LSR_PE: u8 = 1 << 2;
/// LSR bit 3: Framing Error (FE).
pub const LSR_FE: u8 = 1 << 3;
/// LSR bit 4: Break Interrupt (BI).
pub const LSR_BI: u8 = 1 << 4;
/// LSR bit 5: Transmitter Holding Register Empty (THRE).
pub const LSR_THRE: u8 = 1 << 5;
/// LSR bit 6: Transmitter Empty (TEMT).
pub const LSR_TEMT: u8 = 1 << 6;
/// LSR bit 7: Error in RX FIFO (FIFOERR).
pub const LSR_FIFOERR: u8 = 1 << 7;

/// MSR bit 0: Delta Clear to Send (DCTS).
pub const MSR_DCTS: u8 = 1 << 0;
/// MSR bit 1: Delta Data Set Ready (DDSR).
pub const MSR_DDSR: u8 = 1 << 1;
/// MSR bit 2: Trailing Edge Ring Indicator (TERI).
pub const MSR_TERI: u8 = 1 << 2;
/// MSR bit 3: Delta Data Carrier Detect (DDCD).
pub const MSR_DDCD: u8 = 1 << 3;
/// MSR bit 4: Clear to Send (CTS).
pub const MSR_CTS: u8 = 1 << 4;
/// MSR bit 5: Data Set Ready (DSR).
pub const MSR_DSR: u8 = 1 << 5;
/// MSR bit 6: Ring Indicator (RI).
pub const MSR_RI: u8 = 1 << 6;
/// MSR bit 7: Data Carrier Detect (DCD).
pub const MSR_DCD: u8 = 1 << 7;

/// Maximum FIFO depth in 16550 mode.
pub const FIFO_DEPTH: usize = 16;

/// Configuration for the AXI UART 16550 model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uart16550Config {
    /// AXI clock frequency in Hz (`C_S_AXI_CLK_FREQ_HZ`).
    pub clk_freq_hz: u64,
    /// 16550 mode (`true`) or legacy 16450 mode (`false`) (`C_IS_A_16550`).
    pub is_16550: bool,
    /// Whether an external XIN clock is used (`C_HAS_EXTERNAL_XIN`).
    pub has_external_xin: bool,
    /// Address window size in bytes (defaults to 64 KiB, `0x10000`).
    pub window: u64,
}

impl Default for Uart16550Config {
    fn default() -> Self {
        Self {
            clk_freq_hz: 100_000_000,
            is_16550: true,
            has_external_xin: false,
            window: 0x1_0000,
        }
    }
}

impl Uart16550Config {
    /// Parse configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<Self, String> {
        let clk_freq_hz = if p.contains_key("C_S_AXI_CLK_FREQ_HZ") {
            number(p, "C_S_AXI_CLK_FREQ_HZ", 100_000_000)?
        } else {
            number(p, "C_S_AXI_ACLK_FREQ_HZ", 100_000_000)?
        };
        let is_16550 = flag(p, "C_IS_A_16550", true)?;
        let has_external_xin = flag(p, "C_HAS_EXTERNAL_XIN", false)?;
        Ok(Self {
            clk_freq_hz,
            is_16550,
            has_external_xin,
            window,
        })
    }
}

/// Run-time statistics for UART 16550.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uart16550Stats {
    /// Number of bytes transmitted.
    pub tx_bytes: u64,
    /// Number of bytes received.
    pub rx_bytes: u64,
    /// Number of receiver overrun errors.
    pub overrun_errors: u64,
}

struct State {
    cfg: Uart16550Config,
    // Registers
    dll: u8,
    dlm: u8,
    ier: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    msr: u8,
    scr: u8,
    // Error flags for LSR
    oe: bool,
    pe: bool,
    fe: bool,
    bi: bool,
    // Transmitter holding register empty interrupt pending
    thre_pending: bool,
    // Loopback modem state tracking for deltas
    last_loop_cts: bool,
    last_loop_dsr: bool,
    last_loop_ri: bool,
    last_loop_dcd: bool,
    // Queues
    rx_fifo: VecDeque<u8>,
    tx_fifo: VecDeque<u8>,
    tx_collected: Vec<u8>,
    // Statistics
    stats: Uart16550Stats,
    now_ns: u64,
}

impl State {
    fn new(cfg: Uart16550Config) -> Self {
        let mut s = Self {
            cfg,
            dll: 0,
            dlm: 0,
            ier: 0,
            fcr: 0,
            lcr: 0,
            mcr: 0,
            msr: 0,
            scr: 0,
            oe: false,
            pe: false,
            fe: false,
            bi: false,
            thre_pending: true,
            last_loop_cts: false,
            last_loop_dsr: false,
            last_loop_ri: false,
            last_loop_dcd: false,
            rx_fifo: VecDeque::new(),
            tx_fifo: VecDeque::new(),
            tx_collected: Vec::new(),
            stats: Uart16550Stats::default(),
            now_ns: 0,
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.dll = 0;
        self.dlm = 0;
        self.ier = 0;
        self.fcr = 0;
        self.lcr = 0;
        self.mcr = 0;
        self.msr = 0;
        self.scr = 0;
        self.oe = false;
        self.pe = false;
        self.fe = false;
        self.bi = false;
        self.thre_pending = true;
        self.last_loop_cts = false;
        self.last_loop_dsr = false;
        self.last_loop_ri = false;
        self.last_loop_dcd = false;
        self.rx_fifo.clear();
        self.tx_fifo.clear();
        self.tx_collected.clear();
        self.stats = Uart16550Stats::default();
    }

    fn fifo_enabled(&self) -> bool {
        self.cfg.is_16550 && (self.fcr & FCR_FIFO_EN != 0)
    }

    fn fifo_capacity(&self) -> usize {
        if self.fifo_enabled() { FIFO_DEPTH } else { 1 }
    }

    fn rx_trigger(&self) -> usize {
        if !self.fifo_enabled() {
            return 1;
        }
        match (self.fcr >> 6) & 0x03 {
            0 => 1,
            1 => 4,
            2 => 8,
            3 => 14,
            _ => 1,
        }
    }

    fn internal_irq_pending(&self) -> bool {
        let line_status = (self.ier & IER_ELSI != 0) && (self.oe || self.pe || self.fe || self.bi);
        let rx_data = (self.ier & IER_ERBFI != 0) && (self.rx_fifo.len() >= self.rx_trigger());
        let thre = (self.ier & IER_ETBEI != 0) && self.thre_pending;
        let modem = (self.ier & IER_EDSSI != 0) && ((self.msr & 0x0F) != 0);
        line_status || rx_data || thre || modem
    }

    fn irq_active(&self) -> bool {
        // In 16550 UARTs, MCR bit 3 (OUT2) gates the external interrupt line ip2intc_irpt
        (self.mcr & MCR_OUT2 != 0) && self.internal_irq_pending()
    }

    fn read_iir(&mut self) -> u32 {
        let fifo_bits = if self.fifo_enabled() { IIR_FIFO_EN } else { 0 };

        // Priority 1: Receiver Line Status
        if (self.ier & IER_ELSI != 0) && (self.oe || self.pe || self.fe || self.bi) {
            return u32::from(fifo_bits | IIR_LINE_STATUS);
        }

        // Priority 2: Received Data Available
        if (self.ier & IER_ERBFI != 0) && (self.rx_fifo.len() >= self.rx_trigger()) {
            return u32::from(fifo_bits | IIR_RX_DATA);
        }

        // Priority 3: Transmitter Holding Register Empty
        if (self.ier & IER_ETBEI != 0) && self.thre_pending {
            // Reading IIR clears THRE interrupt condition
            self.thre_pending = false;
            return u32::from(fifo_bits | IIR_THRE);
        }

        // Priority 4: Modem Status
        if (self.ier & IER_EDSSI != 0) && ((self.msr & 0x0F) != 0) {
            return u32::from(fifo_bits | IIR_MODEM_STATUS);
        }

        // No interrupt pending
        u32::from(fifo_bits | IIR_NO_INT)
    }

    fn read_lsr(&mut self) -> u32 {
        let mut v = 0u8;
        if !self.rx_fifo.is_empty() {
            v |= LSR_DR;
        }
        if self.oe {
            v |= LSR_OE;
        }
        if self.pe {
            v |= LSR_PE;
        }
        if self.fe {
            v |= LSR_FE;
        }
        if self.bi {
            v |= LSR_BI;
        }
        // Transmitter holding register is empty
        v |= LSR_THRE;
        // Transmitter holding register and shift register are empty
        v |= LSR_TEMT;
        if self.oe || self.pe || self.fe || self.bi {
            v |= LSR_FIFOERR;
        }

        // Reading LSR clears line error flags
        self.oe = false;
        self.pe = false;
        self.fe = false;
        self.bi = false;

        u32::from(v)
    }

    fn read_msr(&mut self) -> u32 {
        let v = self.msr;
        // Reading MSR clears delta bits 3..0
        self.msr &= 0xF0;
        u32::from(v)
    }

    fn write_thr(&mut self, val: u8) {
        self.stats.tx_bytes += 1;
        if self.mcr & MCR_LOOP != 0 {
            // Internal loopback mode
            let cap = self.fifo_capacity();
            if self.rx_fifo.len() < cap {
                self.rx_fifo.push_back(val);
            } else {
                self.oe = true;
                self.stats.overrun_errors += 1;
            }
        } else {
            self.tx_collected.push(val);
        }
        // Character completed transmission; THR remains ready
        self.thre_pending = true;
    }

    fn write_mcr(&mut self, val: u8) {
        let old_mcr = self.mcr;
        self.mcr = val & 0x1F;

        if self.mcr & MCR_LOOP != 0 {
            // Update looped modem status inputs
            let cur_cts = (self.mcr & MCR_RTS) != 0;
            let cur_dsr = (self.mcr & MCR_DTR) != 0;
            let cur_ri = (self.mcr & MCR_OUT1) != 0;
            let cur_dcd = (self.mcr & MCR_OUT2) != 0;

            if (old_mcr & MCR_LOOP == 0) || cur_cts != self.last_loop_cts {
                self.msr |= MSR_DCTS;
                self.last_loop_cts = cur_cts;
            }
            if (old_mcr & MCR_LOOP == 0) || cur_dsr != self.last_loop_dsr {
                self.msr |= MSR_DDSR;
                self.last_loop_dsr = cur_dsr;
            }
            if (!self.last_loop_ri && cur_ri) || (old_mcr & MCR_LOOP == 0 && cur_ri) {
                self.msr |= MSR_TERI;
            }
            self.last_loop_ri = cur_ri;
            if (old_mcr & MCR_LOOP == 0) || cur_dcd != self.last_loop_dcd {
                self.msr |= MSR_DDCD;
                self.last_loop_dcd = cur_dcd;
            }

            self.msr = (self.msr & 0x0F)
                | (if cur_cts { MSR_CTS } else { 0 })
                | (if cur_dsr { MSR_DSR } else { 0 })
                | (if cur_ri { MSR_RI } else { 0 })
                | (if cur_dcd { MSR_DCD } else { 0 });
        }
    }
}

/// An AXI UART 16550 instance implementing [`RegisterBlock`].
pub struct AxiUart16550 {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Handle for host inspection and external UART interaction.
#[derive(Clone)]
pub struct Uart16550Handle {
    st: Arc<Mutex<State>>,
}

impl AxiUart16550 {
    /// Create a new AXI UART 16550 model instance.
    pub fn new(name: &str, cfg: Uart16550Config) -> (Self, Uart16550Handle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            Self {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            Uart16550Handle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Uart16550Handle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Inject bytes into the UART receiver FIFO from the host side.
    pub fn push_rx(&self, data: &[u8]) -> usize {
        let mut s = self.lock();
        let cap = s.fifo_capacity();
        let mut count = 0;
        for &b in data {
            if s.rx_fifo.len() < cap {
                s.rx_fifo.push_back(b);
                count += 1;
            } else {
                s.oe = true;
                s.stats.overrun_errors += 1;
                break;
            }
        }
        count
    }

    /// Take and clear all transmitted bytes collected from the UART transmitter.
    pub fn take_tx(&self) -> Vec<u8> {
        let mut s = self.lock();
        std::mem::take(&mut s.tx_collected)
    }

    /// Current operational statistics.
    pub fn stats(&self) -> Uart16550Stats {
        self.lock().stats
    }

    /// Current interrupt line level.
    pub fn irq(&self) -> bool {
        self.lock().irq_active()
    }

    /// Model configuration.
    pub fn config(&self) -> Uart16550Config {
        self.lock().cfg.clone()
    }
}

impl RegisterBlock for AxiUart16550 {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        self.lock().now_ns = now_ns;
    }

    fn irq_level(&self) -> bool {
        self.irq_output(0)
    }

    fn irq_output(&self, output: usize) -> bool {
        if output == 0 {
            self.lock().irq_active()
        } else {
            false
        }
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let mut s = self.lock();
        match offset {
            RBR_OFFSET => {
                if s.lcr & LCR_DLAB != 0 {
                    u32::from(s.dll)
                } else if let Some(b) = s.rx_fifo.pop_front() {
                    s.stats.rx_bytes += 1;
                    u32::from(b)
                } else {
                    0
                }
            }
            IER_OFFSET => {
                if s.lcr & LCR_DLAB != 0 {
                    u32::from(s.dlm)
                } else {
                    u32::from(s.ier)
                }
            }
            IIR_OFFSET => s.read_iir(),
            LCR_OFFSET => u32::from(s.lcr),
            MCR_OFFSET => u32::from(s.mcr),
            LSR_OFFSET => s.read_lsr(),
            MSR_OFFSET => s.read_msr(),
            SCR_OFFSET => u32::from(s.scr),
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        if strobe & 1 == 0 {
            return;
        }
        let val8 = value as u8;
        let mut s = self.lock();
        match offset {
            THR_OFFSET => {
                if s.lcr & LCR_DLAB != 0 {
                    s.dll = val8;
                } else {
                    s.write_thr(val8);
                }
            }
            IER_OFFSET => {
                if s.lcr & LCR_DLAB != 0 {
                    s.dlm = val8;
                } else {
                    s.ier = val8 & 0x0F;
                    if (s.ier & IER_ETBEI != 0) && s.tx_fifo.is_empty() {
                        s.thre_pending = true;
                    }
                }
            }
            FCR_OFFSET => {
                s.fcr = val8;
                if val8 & FCR_RX_RST != 0 {
                    s.rx_fifo.clear();
                }
                if val8 & FCR_TX_RST != 0 {
                    s.tx_fifo.clear();
                }
            }
            LCR_OFFSET => {
                s.lcr = val8;
            }
            MCR_OFFSET => {
                s.write_mcr(val8);
            }
            SCR_OFFSET => {
                s.scr = val8;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state_and_scratchpad() {
        let (mut dev, handle) = AxiUart16550::new("uart0", Uart16550Config::default());
        assert_eq!(dev.size(), 0x1_0000);
        assert!(!dev.irq_level());
        assert!(!handle.irq());

        // Initial LSR: THRE and TEMT set (0x60)
        assert_eq!(dev.read32(LSR_OFFSET), 0x60);
        // Initial IIR: No interrupt pending (0x01)
        assert_eq!(dev.read32(IIR_OFFSET), 0x01);

        // Scratchpad read/write
        dev.write32(SCR_OFFSET, 0xA5, 0xF);
        assert_eq!(dev.read32(SCR_OFFSET), 0xA5);
        dev.write32(SCR_OFFSET, 0x5A, 0xF);
        assert_eq!(dev.read32(SCR_OFFSET), 0x5A);
    }

    #[test]
    fn divisor_latch_dlab_access() {
        let (mut dev, _) = AxiUart16550::new("uart0", Uart16550Config::default());

        // Set DLAB in LCR
        dev.write32(LCR_OFFSET, u32::from(LCR_DLAB), 0xF);
        assert_eq!(dev.read32(LCR_OFFSET), u32::from(LCR_DLAB));

        // Write DLL and DLM
        dev.write32(DLL_OFFSET, 0x1B, 0xF);
        dev.write32(DLM_OFFSET, 0x02, 0xF);
        assert_eq!(dev.read32(DLL_OFFSET), 0x1B);
        assert_eq!(dev.read32(DLM_OFFSET), 0x02);

        // Clear DLAB and configure 8N1
        dev.write32(LCR_OFFSET, 0x03, 0xF);
        assert_eq!(dev.read32(LCR_OFFSET), 0x03);

        // Now offset 0x1000 is RBR (empty -> 0) and 0x1004 is IER (0)
        assert_eq!(dev.read32(RBR_OFFSET), 0x00);
        assert_eq!(dev.read32(IER_OFFSET), 0x00);
    }

    #[test]
    fn loopback_mode_transmit_and_receive() {
        let (mut dev, handle) = AxiUart16550::new("uart0", Uart16550Config::default());

        // Enable FIFOs and clear them
        dev.write32(
            FCR_OFFSET,
            u32::from(FCR_FIFO_EN | FCR_RX_RST | FCR_TX_RST),
            0xF,
        );
        // Enable Loopback in MCR
        dev.write32(MCR_OFFSET, u32::from(MCR_LOOP), 0xF);

        // Send 'H', 'i'
        dev.write32(THR_OFFSET, u32::from(b'H'), 0xF);
        dev.write32(THR_OFFSET, u32::from(b'i'), 0xF);

        // Check LSR has Data Ready (DR = 1)
        let lsr = dev.read32(LSR_OFFSET);
        assert_ne!(lsr & u32::from(LSR_DR), 0);

        // Read received bytes
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'H'));
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'i'));

        // FIFO should now be empty, DR cleared
        let lsr2 = dev.read32(LSR_OFFSET);
        assert_eq!(lsr2 & u32::from(LSR_DR), 0);

        let stats = handle.stats();
        assert_eq!(stats.tx_bytes, 2);
        assert_eq!(stats.rx_bytes, 2);
    }

    #[test]
    fn fifo_reset_and_overrun() {
        let (mut dev, handle) = AxiUart16550::new("uart0", Uart16550Config::default());

        dev.write32(FCR_OFFSET, u32::from(FCR_FIFO_EN), 0xF);
        dev.write32(MCR_OFFSET, u32::from(MCR_LOOP), 0xF);

        // Fill FIFO with 16 bytes
        for i in 0..16 {
            dev.write32(THR_OFFSET, i, 0xF);
        }
        let lsr = dev.read32(LSR_OFFSET);
        assert_eq!(lsr & u32::from(LSR_OE), 0);

        // 17th byte causes overrun
        dev.write32(THR_OFFSET, 16, 0xF);
        let lsr_ov = dev.read32(LSR_OFFSET);
        assert_ne!(lsr_ov & u32::from(LSR_OE), 0);
        assert_eq!(handle.stats().overrun_errors, 1);

        // Next LSR read should have OE cleared
        let lsr_cleared = dev.read32(LSR_OFFSET);
        assert_eq!(lsr_cleared & u32::from(LSR_OE), 0);

        // Reset RX FIFO
        dev.write32(FCR_OFFSET, u32::from(FCR_FIFO_EN | FCR_RX_RST), 0xF);
        assert_eq!(dev.read32(RBR_OFFSET), 0);
    }

    #[test]
    fn interrupts_thre_and_rx_data_and_out2_gate() {
        let (mut dev, _) = AxiUart16550::new("uart0", Uart16550Config::default());

        // Without OUT2, external interrupt is held low even if THRE enabled
        dev.write32(IER_OFFSET, u32::from(IER_ETBEI), 0xF);
        assert!(!dev.irq_level());
        // Internal IIR shows THRE interrupt
        assert_eq!(dev.read32(IIR_OFFSET), u32::from(IIR_THRE));

        // Set OUT2 and loopback in MCR
        dev.write32(MCR_OFFSET, u32::from(MCR_OUT2 | MCR_LOOP), 0xF);
        // Re-enable IER_ETBEI so THRE interrupt is pending
        dev.write32(IER_OFFSET, u32::from(IER_ETBEI), 0xF);
        assert!(dev.irq_level());

        // Reading IIR clears THRE interrupt
        assert_eq!(dev.read32(IIR_OFFSET), u32::from(IIR_THRE));
        assert!(!dev.irq_level());

        // Enable RX Data Available interrupt (IER_ERBFI)
        dev.write32(IER_OFFSET, u32::from(IER_ERBFI), 0xF);
        assert!(!dev.irq_level());

        // Write a byte in loopback mode
        dev.write32(THR_OFFSET, u32::from(b'X'), 0xF);
        assert!(dev.irq_level());
        assert_eq!(dev.read32(IIR_OFFSET), u32::from(IIR_RX_DATA));

        // Read byte from RBR -> interrupt clears
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'X'));
        assert!(!dev.irq_level());
        assert_eq!(dev.read32(IIR_OFFSET), u32::from(IIR_NO_INT));
    }

    #[test]
    fn modem_status_and_deltas_in_loopback() {
        let (mut dev, _) = AxiUart16550::new("uart0", Uart16550Config::default());

        // Set Loopback, RTS and DTR in MCR
        dev.write32(MCR_OFFSET, u32::from(MCR_LOOP | MCR_RTS | MCR_DTR), 0xF);

        // MSR should reflect CTS, DSR and their deltas DCTS, DDSR
        let msr = dev.read32(MSR_OFFSET);
        assert_ne!(msr & u32::from(MSR_CTS), 0);
        assert_ne!(msr & u32::from(MSR_DSR), 0);
        assert_ne!(msr & u32::from(MSR_DCTS), 0);
        assert_ne!(msr & u32::from(MSR_DDSR), 0);

        // Read again: deltas should be cleared
        let msr2 = dev.read32(MSR_OFFSET);
        assert_eq!(msr2 & 0x0F, 0);
    }

    #[test]
    fn host_handle_push_rx_and_take_tx() {
        let (mut dev, handle) = AxiUart16550::new("uart0", Uart16550Config::default());

        // Normal mode (no loopback): writes to THR collect into tx_collected
        dev.write32(THR_OFFSET, u32::from(b'O'), 0xF);
        dev.write32(THR_OFFSET, u32::from(b'K'), 0xF);

        let tx = handle.take_tx();
        assert_eq!(tx, b"OK");
        assert!(handle.take_tx().is_empty());

        // Enable FIFOs so FIFO capacity is 16
        dev.write32(FCR_OFFSET, u32::from(FCR_FIFO_EN), 0xF);

        // Host pushes into RX FIFO
        let pushed = handle.push_rx(b"PING");
        assert_eq!(pushed, 4);

        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'P'));
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'I'));
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'N'));
        assert_eq!(dev.read32(RBR_OFFSET), u32::from(b'G'));
        assert_eq!(dev.read32(RBR_OFFSET), 0);
    }
}
