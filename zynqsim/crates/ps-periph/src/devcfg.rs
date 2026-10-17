//! The device configuration block (devcfg, UG585 ch.6) at 0xF8007000 and the XADC interface
//! (xadcps, UG480 as the interface is used through the PS) at 0xF8007100, which lies inside the
//! same 4 KiB window. One device serves both: offsets 0x000..=0x0FF are devcfg, 0x100..=0x1FF
//! are the XADC interface.
//!
//! Devcfg. The register offsets are `VERIFY:` against UG585 (CTRL 0x00, LOCK 0x04, CFG 0x08,
//! INT_STS 0x0C, INT_MASK 0x10, STATUS 0x14, DMA source and destination 0x18/0x1C, their lengths
//! 0x20/0x24, ROM_SHADOW 0x28, SW_ID 0x30, UNLOCK 0x34, MCTRL 0x80). The model keeps what the
//! driver writes and reads back: `XDcfg_SelfTest` (`bsp_devcfg_selftest`, disassembled from the
//! ELF) writes CTRL bit 4 (NIDEN) and reads it back; the self test passes when the bit sticks.
//! No bitstream is loaded and no DMA runs: DMA_SRC/DST/LEN are stored only. Interrupt ID 40 is
//! `VERIFY:`, and its status is INT_STS masked by INT_MASK (no event is produced).
//!
//! XADC. Command word through CMDFIFO (VERIFY: the command layout as the Linux `xilinx-xadc`
//! driver uses it): bits 27:26 command (1 read, 2 write), 25:16 DRP address, 15:0 data. A read
//! pushes its 16-bit result into RDFIFO; a write pushes nothing. The on-chip sensors live at
//! DRP 0x00 (temperature) and 0x01 (VCCINT) and hold the 12-bit code in bits 15:4, as UG480
//! describes. The host sets the physical values; the model converts them with the UG480 transfer
//! functions, T = code * 503.975 / 4096 - 273.15 and V = code * 3 / 4096. DRP 0x40..=0x7F are
//! read/write. The XADC interrupt is not modelled.

use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::VecDeque;

/// GIC interrupt ID of the device configuration interface (`VERIFY:`).
pub const DEVCFG_IRQ: u16 = 40;
/// Depth of the XADC read FIFO.
pub const RDFIFO_DEPTH: usize = 16;
/// The value the BSP writes to UNLOCK (unlock key).
pub const UNLOCK_KEY: u32 = 0x757B_DF0D;

/// Reset value of CTRL (VERIFY).
const CTRL_RESET: u32 = 0x8000_0000;
/// CTRL bit 4: NIDEN, exercised by the BSP self test.
pub const CTRL_NIDEN: u32 = 1 << 4;
/// Reset value of STATUS: PCFG_INIT high (VERIFY).
const STATUS_RESET: u32 = 0x0000_0010;

const DRP_TEMP: usize = 0x00;
const DRP_VCCINT: usize = 0x01;
const DRP_VCCAUX: usize = 0x02;
const DRP_RW_BASE: usize = 0x40;
const DRP_SIZE: usize = 0x80;

/// The UG480 transfer functions.
pub fn code_to_celsius(code: u16) -> f64 {
    f64::from(code) * 503.975 / 4096.0 - 273.15
}

/// The UG480 supply transfer function (volts).
pub fn code_to_volts(code: u16) -> f64 {
    f64::from(code) * 3.0 / 4096.0
}

/// The 12-bit code of a temperature in degrees Celsius, clamped to the converter range.
pub fn celsius_to_code(c: f64) -> u16 {
    ((c + 273.15) * 4096.0 / 503.975).round().clamp(0.0, 4095.0) as u16
}

/// The 12-bit code of a supply voltage in volts, clamped to the converter range.
pub fn volts_to_code(v: f64) -> u16 {
    (v * 4096.0 / 3.0).round().clamp(0.0, 4095.0) as u16
}

/// The XADC interface with its DRP map and FIFOs.
struct Xadc {
    cfg: u32,
    int_sts: u32,
    int_mask: u32,
    mctl: u32,
    drp: Vec<u16>,
    rdfifo: VecDeque<u16>,
}

impl Xadc {
    fn new() -> Xadc {
        let mut drp = vec![0u16; DRP_SIZE];
        drp[DRP_TEMP] = celsius_to_code(25.0) << 4;
        drp[DRP_VCCINT] = volts_to_code(1.0) << 4;
        drp[DRP_VCCAUX] = volts_to_code(1.8) << 4;
        Xadc {
            cfg: 0,
            int_sts: 0,
            int_mask: 0,
            mctl: 0,
            drp,
            rdfifo: VecDeque::new(),
        }
    }

    /// One command word from CMDFIFO.
    fn command(&mut self, word: u32) {
        let cmd = (word >> 26) & 0x3;
        let addr = ((word >> 16) & 0x3FF) as usize;
        let data = (word & 0xFFFF) as u16;
        match cmd {
            1 => {
                let v = self.drp.get(addr).copied().unwrap_or(0);
                if self.rdfifo.len() < RDFIFO_DEPTH {
                    self.rdfifo.push_back(v);
                }
            }
            2 => {
                if (DRP_RW_BASE..DRP_SIZE).contains(&addr) {
                    self.drp[addr] = data;
                }
            }
            _ => {}
        }
    }

    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let word = match offset & !3 {
            0x00 => self.cfg,
            0x04 => self.int_sts,
            0x08 => self.int_mask,
            0x0C => 0, // MSTS: no status bits in this model
            0x10 => 0, // CMDFIFO is write-only
            0x14 => u32::from(self.rdfifo.pop_front().unwrap_or(0)),
            0x18 => self.mctl,
            a if a < 0x100 => 0,
            _ => return Err(BusFault::Unmapped(offset)),
        };
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        match offset & !3 {
            0x00 => {
                let (_, w) = lane_merge(offset, size, value, self.cfg)?;
                self.cfg = w;
            }
            0x04 => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.int_sts &= !(w & 0x3FF);
            }
            0x08 => {
                let (_, w) = lane_merge(offset, size, value, self.int_mask)?;
                self.int_mask = w & 0x3FF;
            }
            0x10 => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.command(w);
            }
            0x18 => {
                // MCTL bit 4 (reset) is self clearing.
                let (_, w) = lane_merge(offset, size, value, self.mctl)?;
                self.mctl = w & 0xFF & !0x10;
            }
            a if a < 0x100 => {
                lane_merge(offset, size, value, 0)?;
            }
            _ => return Err(BusFault::Unmapped(offset)),
        }
        Ok(())
    }
}

/// The devcfg block and the XADC interface.
pub struct DevCfg {
    ctrl: u32,
    lock: u32,
    cfg: u32,
    int_sts: u32,
    int_mask: u32,
    dma_src: u32,
    dma_dst: u32,
    dma_src_len: u32,
    dma_dst_len: u32,
    rom_shadow: u32,
    sw_id: u32,
    mctrl: u32,
    xadc: Xadc,
}

impl Default for DevCfg {
    fn default() -> Self {
        DevCfg::new()
    }
}

impl DevCfg {
    /// Both blocks in reset state; the XADC sensors read 25 C and 1.0 V (host values).
    pub fn new() -> DevCfg {
        DevCfg {
            ctrl: CTRL_RESET,
            lock: 0,
            cfg: 0,
            int_sts: 0,
            int_mask: u32::MAX,
            dma_src: 0,
            dma_dst: 0,
            dma_src_len: 0,
            dma_dst_len: 0,
            rom_shadow: 0,
            sw_id: 0,
            mctrl: 0,
            xadc: Xadc::new(),
        }
    }

    /// Sets the die temperature the XADC converts (degrees Celsius; host side).
    pub fn set_temperature_c(&mut self, c: f64) {
        self.xadc.drp[DRP_TEMP] = celsius_to_code(c) << 4;
    }

    /// Sets the VCCINT the XADC converts (volts; host side).
    pub fn set_vccint_v(&mut self, v: f64) {
        self.xadc.drp[DRP_VCCINT] = volts_to_code(v) << 4;
    }

    /// The raw 16-bit DRP value at `addr` (for tests and the host).
    pub fn drp(&self, addr: usize) -> u16 {
        self.xadc.drp.get(addr).copied().unwrap_or(0)
    }

    fn devcfg_read(&self, a: u64) -> u32 {
        match a {
            0x00 => self.ctrl,
            0x04 => self.lock,
            0x08 => self.cfg,
            0x0C => self.int_sts,
            0x10 => self.int_mask,
            0x14 => STATUS_RESET,
            0x18 => self.dma_src,
            0x1C => self.dma_dst,
            0x20 => self.dma_src_len,
            0x24 => self.dma_dst_len,
            0x28 => self.rom_shadow,
            0x30 => self.sw_id,
            0x80 => self.mctrl,
            _ => 0,
        }
    }

    fn devcfg_write(&mut self, offset: u64, size: u8, value: u64, a: u64) -> BusResult<()> {
        let word = match a {
            0x00 => &mut self.ctrl,
            0x04 => &mut self.lock,
            0x08 => &mut self.cfg,
            0x0C => {
                // Write 1 to clear.
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.int_sts &= !w;
                return Ok(());
            }
            0x10 => &mut self.int_mask,
            0x18 => &mut self.dma_src,
            0x1C => &mut self.dma_dst,
            0x20 => &mut self.dma_src_len,
            0x24 => &mut self.dma_dst_len,
            0x28 => &mut self.rom_shadow,
            0x30 => &mut self.sw_id,
            0x34 => {
                // UNLOCK takes the key; the model keeps no lock state beyond the register.
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.lock = u32::from(w == UNLOCK_KEY);
                return Ok(());
            }
            0x80 => &mut self.mctrl,
            _ => {
                lane_merge(offset, size, value, 0)?;
                return Ok(());
            }
        };
        let (_, w) = lane_merge(offset, size, value, *word)?;
        *word = w;
        Ok(())
    }
}

impl Device for DevCfg {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        if offset >= 0x100 && offset < 0x200 {
            return self.xadc.read(offset - 0x100, size);
        }
        if offset >= 0x200 {
            lane_read(offset, size, 0)?;
            return Ok(0);
        }
        let word = self.devcfg_read(offset & !3);
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        if (0x100..0x200).contains(&offset) {
            return self.xadc.write(offset - 0x100, size, value);
        }
        if offset >= 0x200 {
            lane_merge(offset, size, value, 0)?;
            return Ok(());
        }
        self.devcfg_write(offset, size, value, offset & !3)
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: DEVCFG_IRQ,
            cpu: None,
            level: self.int_sts & !self.int_mask != 0,
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for v in [
            self.ctrl,
            self.lock,
            self.cfg,
            self.int_sts,
            self.int_mask,
            self.dma_src,
            self.dma_dst,
            self.dma_src_len,
            self.dma_dst_len,
            self.rom_shadow,
            self.sw_id,
            self.mctrl,
            self.xadc.cfg,
            self.xadc.int_sts,
            self.xadc.int_mask,
            self.xadc.mctl,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.xadc.drp {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.xadc.rdfifo.len() as u32).to_le_bytes());
        for v in &self.xadc.rdfifo {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wr(d: &mut DevCfg, off: u64, v: u32) {
        d.write(off, 4, u64::from(v)).unwrap();
    }

    fn rd(d: &mut DevCfg, off: u64) -> u32 {
        d.read(off, 4).unwrap() as u32
    }

    fn cmd(cmd: u32, addr: u32, data: u32) -> u32 {
        (cmd << 26) | (addr << 16) | (data & 0xFFFF)
    }

    /// One DRP read through the command FIFO, as the firmware does it.
    fn drp_read(d: &mut DevCfg, addr: u32) -> u32 {
        wr(d, 0x100 + 0x10, cmd(1, addr, 0));
        rd(d, 0x100 + 0x14)
    }

    #[test]
    fn ctrl_nidien_reads_back_as_written_like_the_bsp_self_test_needs() {
        let mut d = DevCfg::new();
        let before = rd(&mut d, 0x00);
        wr(&mut d, 0x00, before | CTRL_NIDEN);
        assert_ne!(rd(&mut d, 0x00) & CTRL_NIDEN, 0);
        wr(&mut d, 0x00, before);
        assert_eq!(rd(&mut d, 0x00), before);
    }

    #[test]
    fn devcfg_registers_keep_what_the_driver_writes() {
        let mut d = DevCfg::new();
        wr(&mut d, 0x18, 0x0010_0000);
        wr(&mut d, 0x20, 0x40);
        assert_eq!(rd(&mut d, 0x18), 0x0010_0000);
        assert_eq!(rd(&mut d, 0x20), 0x40);
        assert_eq!(rd(&mut d, 0x14) & 0x10, 0x10, "PCFG_INIT reads high");
        wr(&mut d, 0x34, UNLOCK_KEY);
        assert_eq!(rd(&mut d, 0x04), 1, "unlocked by the key");
    }

    #[test]
    fn interrupt_status_is_write_one_to_clear_and_masked_by_the_mask() {
        let mut d = DevCfg::new();
        d.int_sts = 0b101;
        let mut lines = Vec::new();
        d.irq_lines(&mut lines);
        assert!(!lines[0].level, "all masked at reset");
        wr(&mut d, 0x10, 0);
        lines.clear();
        d.irq_lines(&mut lines);
        assert_eq!(lines[0], IrqInput { id: 40, cpu: None, level: true });
        wr(&mut d, 0x0C, 0b100);
        assert_eq!(rd(&mut d, 0x0C), 0b001);
    }

    #[test]
    fn xadc_temperature_and_vccint_come_back_through_the_fifos() {
        let mut d = DevCfg::new();
        let t = drp_read(&mut d, DRP_TEMP as u32);
        let c = code_to_celsius((t >> 4) as u16);
        assert!((c - 25.0).abs() < 0.2, "{c}");
        let v = drp_read(&mut d, DRP_VCCINT as u32);
        let volts = code_to_volts((v >> 4) as u16);
        assert!((volts - 1.0).abs() < 0.01, "{volts}");
        d.set_temperature_c(61.5);
        let t2 = drp_read(&mut d, DRP_TEMP as u32);
        assert!((code_to_celsius((t2 >> 4) as u16) - 61.5).abs() < 0.2);
        d.set_vccint_v(0.95);
        let v2 = drp_read(&mut d, DRP_VCCINT as u32);
        assert!((code_to_volts((v2 >> 4) as u16) - 0.95).abs() < 0.01);
    }

    #[test]
    fn xadc_drp_write_then_read_returns_the_value_and_writes_push_nothing() {
        let mut d = DevCfg::new();
        wr(&mut d, 0x100 + 0x10, cmd(2, 0x40, 0x1234));
        assert_eq!(d.xadc.rdfifo.len(), 0, "a write produces no FIFO entry");
        assert_eq!(drp_read(&mut d, 0x40), 0x1234);
        // The sensor registers are read only.
        wr(&mut d, 0x100 + 0x10, cmd(2, DRP_TEMP as u32, 0));
        assert_eq!(d.drp(DRP_TEMP) >> 4, celsius_to_code(25.0));
    }

    #[test]
    fn an_empty_read_fifo_returns_zero_and_reserved_offsets_read_zero() {
        let mut d = DevCfg::new();
        assert_eq!(rd(&mut d, 0x100 + 0x14), 0);
        assert_eq!(rd(&mut d, 0x400), 0);
        assert_eq!(rd(&mut d, 0x0F0), 0);
    }

    #[test]
    fn the_xadc_mctl_reset_bit_is_self_clearing() {
        let mut d = DevCfg::new();
        wr(&mut d, 0x100 + 0x18, 0x40);
        assert_eq!(rd(&mut d, 0x100 + 0x18), 0x40);
        wr(&mut d, 0x100 + 0x18, 0x10);
        assert_eq!(rd(&mut d, 0x100 + 0x18), 0);
    }

    #[test]
    fn conversions_are_inverses_within_one_code() {
        for c in [-40.0, 0.0, 25.0, 85.0, 125.0] {
            let back = code_to_celsius(celsius_to_code(c));
            assert!((back - c).abs() < 0.2, "{c} -> {back}");
        }
        for v in [0.9, 1.0, 1.1] {
            assert!((code_to_volts(volts_to_code(v)) - v).abs() < 0.01);
        }
    }
}
