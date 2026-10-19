//! AXI Central Direct Memory Access (`xilinx.com:ip:axi_cdma:4.1`, Xilinx PG034), register level:
//! memory-to-memory DMA transfers in Simple and Scatter-Gather (SG) modes.
//!
//! Register map (offsets from the base address; from PG034):
//!
//! | offset | register | description |
//! |---|---|---|
//! | `0x00` | `CDMACR` | CDMA Control Register |
//! | `0x04` | `CDMASR` | CDMA Status Register |
//! | `0x08` | `CURDESC_PNTR` | Current Descriptor Pointer (lower 32 bits) |
//! | `0x0C` | `CURDESC_PNTR_MSB` | Current Descriptor Pointer (upper 32 bits) |
//! | `0x10` | `TAILDESC_PNTR` | Tail Descriptor Pointer (lower 32 bits; starts SG transfer) |
//! | `0x14` | `TAILDESC_PNTR_MSB` | Tail Descriptor Pointer (upper 32 bits) |
//! | `0x18` | `SA` | Source Address (lower 32 bits) |
//! | `0x1C` | `SA_MSB` | Source Address (upper 32 bits) |
//! | `0x20` | `DA` | Destination Address (lower 32 bits) |
//! | `0x24` | `DA_MSB` | Destination Address (upper 32 bits) |
//! | `0x28` | `BTT` | Bytes to Transfer (writing starts Simple transfer) |
//!
//! `CDMACR`:
//! * bit 2: `Reset` (soft reset, restores defaults, self-clears)
//! * bit 3: `SGMode` (1 = Scatter Gather mode enabled, 0 = Simple mode)
//! * bit 4: `Keyhole_Read` (read address does not increment)
//! * bit 5: `Keyhole_Write` (write address does not increment)
//! * bit 12: `IOC_IrqEn` (completion interrupt enable)
//! * bit 13: `Dly_IrqEn` (delay interrupt enable)
//! * bit 14: `Err_IrqEn` (error interrupt enable)
//! * bits 23:16: `IRQThreshold`
//! * bits 31:24: `IRQDelay`
//!
//! `CDMASR`:
//! * bit 1: `Idle` (1 = Idle, 0 = Busy)
//! * bit 3: `SGIncld` (read-only; 1 if SG mode included in hardware)
//! * bit 4: `DMAIntErr` (internal error)
//! * bit 5: `DMASlvErr` (slave error)
//! * bit 6: `DMADecErr` (decode error)
//! * bit 8: `SGIntErr` (scatter-gather internal error)
//! * bit 9: `SGSlvErr` (scatter-gather slave error)
//! * bit 10: `SGDecErr` (scatter-gather decode error)
//! * bit 12: `IOC_Irq` (interrupt on completion, W1C)
//! * bit 13: `Dly_Irq` (interrupt on delay, W1C)
//! * bit 14: `Err_Irq` (interrupt on error, W1C)
//!
//! Interrupt line: output 0 is `cdma_introut`. High when any enabled interrupt bit in `CDMASR` is set.

use crate::dma::parse_ps_port;
use crate::params::{Params, flag, ranged};
use pl_api::{BusError, MasterAccess, PsMasterPort, PsPort};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `CDMACR`: CDMA Control Register.
pub const CDMACR: u64 = 0x00;
/// `CDMASR`: CDMA Status Register.
pub const CDMASR: u64 = 0x04;
/// `CURDESC_PNTR`: Current Descriptor Pointer (lower 32 bits).
pub const CURDESC_PNTR: u64 = 0x08;
/// `CURDESC_PNTR_MSB`: Current Descriptor Pointer (upper 32 bits).
pub const CURDESC_PNTR_MSB: u64 = 0x0C;
/// `TAILDESC_PNTR`: Tail Descriptor Pointer (lower 32 bits).
pub const TAILDESC_PNTR: u64 = 0x10;
/// `TAILDESC_PNTR_MSB`: Tail Descriptor Pointer (upper 32 bits).
pub const TAILDESC_PNTR_MSB: u64 = 0x14;
/// `SA`: Source Address (lower 32 bits).
pub const SA: u64 = 0x18;
/// `SA_MSB`: Source Address (upper 32 bits).
pub const SA_MSB: u64 = 0x1C;
/// `DA`: Destination Address (lower 32 bits).
pub const DA: u64 = 0x20;
/// `DA_MSB`: Destination Address (upper 32 bits).
pub const DA_MSB: u64 = 0x24;
/// `BTT`: Bytes to Transfer.
pub const BTT: u64 = 0x28;

/// `CDMACR` Reset bit.
pub const CR_RESET: u32 = 1 << 2;
/// `CDMACR` SGMode bit.
pub const CR_SGMODE: u32 = 1 << 3;
/// `CDMACR` Keyhole Read bit.
pub const CR_KEYHOLE_RD: u32 = 1 << 4;
/// `CDMACR` Keyhole Write bit.
pub const CR_KEYHOLE_WR: u32 = 1 << 5;
/// `CDMACR` Completion Interrupt Enable.
pub const CR_IOC_IRQ_EN: u32 = 1 << 12;
/// `CDMACR` Delay Interrupt Enable.
pub const CR_DLY_IRQ_EN: u32 = 1 << 13;
/// `CDMACR` Error Interrupt Enable.
pub const CR_ERR_IRQ_EN: u32 = 1 << 14;

/// `CDMASR` Idle bit.
pub const SR_IDLE: u32 = 1 << 1;
/// `CDMASR` SGIncld bit.
pub const SR_SGINCLD: u32 = 1 << 3;
/// `CDMASR` Internal Error bit.
pub const SR_ERR_INTERNAL: u32 = 1 << 4;
/// `CDMASR` Slave Error bit.
pub const SR_ERR_SLAVE: u32 = 1 << 5;
/// `CDMASR` Decode Error bit.
pub const SR_ERR_DECODE: u32 = 1 << 6;
/// `CDMASR` SG Internal Error bit.
pub const SR_ERR_SG_INT: u32 = 1 << 8;
/// `CDMASR` SG Slave Error bit.
pub const SR_ERR_SG_SLV: u32 = 1 << 9;
/// `CDMASR` SG Decode Error bit.
pub const SR_ERR_SG_DEC: u32 = 1 << 10;
/// `CDMASR` Completion Interrupt bit.
pub const SR_IOC_IRQ: u32 = 1 << 12;
/// `CDMASR` Delay Interrupt bit.
pub const SR_DLY_IRQ: u32 = 1 << 13;
/// `CDMASR` Error Interrupt bit.
pub const SR_ERR_IRQ: u32 = 1 << 14;

/// Configuration of an AXI CDMA instance.
#[derive(Clone, Debug)]
pub struct CdmaConfig {
    /// Scatter-gather capability (`C_INCLUDE_SG`).
    pub include_sg: bool,
    /// Data Realignment Engine (`C_INCLUDE_DRE`).
    pub include_dre: bool,
    /// Data bus width in bits (`C_M_AXI_DATA_WIDTH`, 32..1024).
    pub data_width: u32,
    /// Address width in bits (`C_M_AXI_ADDR_WIDTH`, 32..64).
    pub addr_width: u32,
    /// Maximum burst length (`C_M_AXI_MAX_BURST_LEN`, 2..256).
    pub max_burst_len: u32,
    /// PS master interface used to access system memory.
    pub ps_port: PsMasterPort,
    /// Address window size in bytes.
    pub window: u64,
}

impl Default for CdmaConfig {
    fn default() -> Self {
        CdmaConfig {
            include_sg: true,
            include_dre: true,
            data_width: 32,
            addr_width: 32,
            max_burst_len: 16,
            ps_port: PsMasterPort::SAxiHp0,
            window: 0x1_0000,
        }
    }
}

impl CdmaConfig {
    /// Parses configuration from IP parameters and window size.
    pub fn from_params(p: &Params, window: u64) -> Result<CdmaConfig, String> {
        let width = ranged(p, "C_M_AXI_DATA_WIDTH", 32, 32, 1024)? as u32;
        if !width.is_power_of_two() {
            return Err(format!(
                "parameter C_M_AXI_DATA_WIDTH = {width} is not a power of two"
            ));
        }
        let addr_w = ranged(p, "C_M_AXI_ADDR_WIDTH", 32, 32, 64)? as u32;
        let burst = ranged(p, "C_M_AXI_MAX_BURST_LEN", 16, 2, 256)? as u32;
        let port = match p.get("PS_PORT") {
            Some(v) => parse_ps_port(v)?,
            None => PsMasterPort::SAxiHp0,
        };
        Ok(CdmaConfig {
            include_sg: flag(p, "C_INCLUDE_SG", true)?,
            include_dre: flag(p, "C_INCLUDE_DRE", true)?,
            data_width: width,
            addr_width: addr_w,
            max_burst_len: burst,
            ps_port: port,
            window,
        })
    }
}

/// Statistics for CDMA operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CdmaStats {
    /// Total number of completed transfers (simple or SG descriptors).
    pub transfers: u64,
    /// Total bytes transferred.
    pub bytes: u64,
    /// Number of completed scatter-gather descriptors.
    pub descriptors: u64,
}

/// Internal state of the CDMA engine.
struct State {
    cfg: CdmaConfig,
    now: u64,

    // Control registers
    sg_mode: bool,
    keyhole_read: bool,
    keyhole_write: bool,
    ioc_irq_en: bool,
    dly_irq_en: bool,
    err_irq_en: bool,
    irq_threshold: u8,
    irq_delay: u8,

    // Status registers
    idle: bool,
    err_internal: bool,
    err_slave: bool,
    err_decode: bool,
    err_sg_int: bool,
    err_sg_slv: bool,
    err_sg_dec: bool,
    ioc_irq: bool,
    dly_irq: bool,
    err_irq: bool,

    // Address & length registers
    curdesc: u64,
    taildesc: u64,
    sa: u64,
    da: u64,
    btt: u32,

    // Pending execution flags
    pending_simple: bool,
    pending_sg: bool,

    log: Vec<MasterAccess>,
    stats: CdmaStats,
}

impl State {
    fn power_on(cfg: CdmaConfig) -> Self {
        State {
            cfg,
            now: 0,
            sg_mode: false,
            keyhole_read: false,
            keyhole_write: false,
            ioc_irq_en: false,
            dly_irq_en: false,
            err_irq_en: false,
            irq_threshold: 1,
            irq_delay: 0,
            idle: true,
            err_internal: false,
            err_slave: false,
            err_decode: false,
            err_sg_int: false,
            err_sg_slv: false,
            err_sg_dec: false,
            ioc_irq: false,
            dly_irq: false,
            err_irq: false,
            curdesc: 0,
            taildesc: 0,
            sa: 0,
            da: 0,
            btt: 0,
            pending_simple: false,
            pending_sg: false,
            log: Vec::new(),
            stats: CdmaStats::default(),
        }
    }

    fn reset(&mut self) {
        self.sg_mode = false;
        self.keyhole_read = false;
        self.keyhole_write = false;
        self.ioc_irq_en = false;
        self.dly_irq_en = false;
        self.err_irq_en = false;
        self.irq_threshold = 1;
        self.irq_delay = 0;
        self.idle = true;
        self.err_internal = false;
        self.err_slave = false;
        self.err_decode = false;
        self.err_sg_int = false;
        self.err_sg_slv = false;
        self.err_sg_dec = false;
        self.ioc_irq = false;
        self.dly_irq = false;
        self.err_irq = false;
        self.curdesc = 0;
        self.taildesc = 0;
        self.sa = 0;
        self.da = 0;
        self.btt = 0;
        self.pending_simple = false;
        self.pending_sg = false;
    }

    fn cr(&self) -> u32 {
        let mut v = 0;
        if self.sg_mode {
            v |= CR_SGMODE;
        }
        if self.keyhole_read {
            v |= CR_KEYHOLE_RD;
        }
        if self.keyhole_write {
            v |= CR_KEYHOLE_WR;
        }
        if self.ioc_irq_en {
            v |= CR_IOC_IRQ_EN;
        }
        if self.dly_irq_en {
            v |= CR_DLY_IRQ_EN;
        }
        if self.err_irq_en {
            v |= CR_ERR_IRQ_EN;
        }
        v |= (self.irq_threshold as u32) << 16;
        v |= (self.irq_delay as u32) << 24;
        v
    }

    fn write_cr(&mut self, val: u32) {
        if val & CR_RESET != 0 {
            self.reset();
            return;
        }
        if self.idle && self.cfg.include_sg {
            self.sg_mode = val & CR_SGMODE != 0;
        }
        self.keyhole_read = val & CR_KEYHOLE_RD != 0;
        self.keyhole_write = val & CR_KEYHOLE_WR != 0;
        self.ioc_irq_en = val & CR_IOC_IRQ_EN != 0;
        self.dly_irq_en = val & CR_DLY_IRQ_EN != 0;
        self.err_irq_en = val & CR_ERR_IRQ_EN != 0;
        let thr = (val >> 16) & 0xFF;
        self.irq_threshold = if thr == 0 { 1 } else { thr as u8 };
        self.irq_delay = (val >> 24) as u8;
    }

    fn sr(&self) -> u32 {
        let mut v = 0;
        if self.idle {
            v |= SR_IDLE;
        }
        if self.cfg.include_sg {
            v |= SR_SGINCLD;
        }
        if self.err_internal {
            v |= SR_ERR_INTERNAL;
        }
        if self.err_slave {
            v |= SR_ERR_SLAVE;
        }
        if self.err_decode {
            v |= SR_ERR_DECODE;
        }
        if self.err_sg_int {
            v |= SR_ERR_SG_INT;
        }
        if self.err_sg_slv {
            v |= SR_ERR_SG_SLV;
        }
        if self.err_sg_dec {
            v |= SR_ERR_SG_DEC;
        }
        if self.ioc_irq {
            v |= SR_IOC_IRQ;
        }
        if self.dly_irq {
            v |= SR_DLY_IRQ;
        }
        if self.err_irq {
            v |= SR_ERR_IRQ;
        }
        v |= (self.irq_threshold as u32) << 16;
        v |= (self.irq_delay as u32) << 24;
        v
    }

    fn write_sr(&mut self, val: u32) {
        // W1C for interrupt and error flags
        if val & SR_IOC_IRQ != 0 {
            self.ioc_irq = false;
        }
        if val & SR_DLY_IRQ != 0 {
            self.dly_irq = false;
        }
        if val & SR_ERR_IRQ != 0 {
            self.err_irq = false;
        }
        if val & SR_ERR_INTERNAL != 0 {
            self.err_internal = false;
        }
        if val & SR_ERR_SLAVE != 0 {
            self.err_slave = false;
        }
        if val & SR_ERR_DECODE != 0 {
            self.err_decode = false;
        }
        if val & SR_ERR_SG_INT != 0 {
            self.err_sg_int = false;
        }
        if val & SR_ERR_SG_SLV != 0 {
            self.err_sg_slv = false;
        }
        if val & SR_ERR_SG_DEC != 0 {
            self.err_sg_dec = false;
        }
    }

    fn read_reg(&self, offset: u64) -> u32 {
        match offset {
            CDMACR => self.cr(),
            CDMASR => self.sr(),
            CURDESC_PNTR => self.curdesc as u32,
            CURDESC_PNTR_MSB => (self.curdesc >> 32) as u32,
            TAILDESC_PNTR => self.taildesc as u32,
            TAILDESC_PNTR_MSB => (self.taildesc >> 32) as u32,
            SA => self.sa as u32,
            SA_MSB => (self.sa >> 32) as u32,
            DA => self.da as u32,
            DA_MSB => (self.da >> 32) as u32,
            BTT => self.btt,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            CDMACR => self.write_cr(value),
            CDMASR => self.write_sr(value),
            CURDESC_PNTR => {
                if self.idle && self.sg_mode {
                    self.curdesc =
                        (self.curdesc & 0xFFFF_FFFF_0000_0000) | (u64::from(value) & !0x3F);
                }
            }
            CURDESC_PNTR_MSB => {
                if self.idle && self.sg_mode {
                    self.curdesc =
                        (self.curdesc & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }
            TAILDESC_PNTR => {
                if self.sg_mode {
                    self.taildesc =
                        (self.taildesc & 0xFFFF_FFFF_0000_0000) | (u64::from(value) & !0x3F);
                    self.idle = false;
                    self.pending_sg = true;
                }
            }
            TAILDESC_PNTR_MSB => {
                if self.sg_mode {
                    self.taildesc =
                        (self.taildesc & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }
            SA => {
                self.sa = (self.sa & 0xFFFF_FFFF_0000_0000) | u64::from(value);
            }
            SA_MSB => {
                self.sa = (self.sa & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
            }
            DA => {
                self.da = (self.da & 0xFFFF_FFFF_0000_0000) | u64::from(value);
            }
            DA_MSB => {
                self.da = (self.da & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
            }
            BTT if !self.sg_mode => {
                let len = value & 0x7F_FFFF;
                self.btt = len;
                self.idle = false;
                self.pending_simple = true;
            }
            _ => {}
        }
    }

    fn record(&mut self, addr: u64, len: usize, write: bool, data: &[u8], ok: bool) {
        let mut d = [0u8; 8];
        let n = len.min(8);
        d[..n].copy_from_slice(&data[..n]);
        self.log.push(MasterAccess {
            t_ns: self.now,
            port: self.cfg.ps_port,
            write,
            addr,
            len: len as u32,
            data: u64::from_le_bytes(d),
            ok,
        });
    }

    fn run(&mut self, ps: &mut dyn PsPort) {
        // 1. Simple mode transfer
        if self.pending_simple {
            let len = self.btt as usize;
            if len == 0 || (!self.cfg.include_dre && (self.sa % 4 != 0 || self.da % 4 != 0)) {
                self.err_internal = true;
                self.err_irq = true;
                self.idle = true;
            } else {
                let mut buf = vec![0u8; len];
                let r_read = if self.keyhole_read {
                    let mut ok = Ok(());
                    for b in buf.iter_mut() {
                        let mut single = [0u8; 1];
                        match ps.mem_read(self.cfg.ps_port, self.sa, &mut single) {
                            Ok(()) => *b = single[0],
                            Err(e) => {
                                ok = Err(e);
                                break;
                            }
                        }
                    }
                    ok
                } else {
                    ps.mem_read(self.cfg.ps_port, self.sa, &mut buf)
                };

                self.record(self.sa, len, false, &buf, r_read.is_ok());

                match r_read {
                    Err(BusError::Decode) => {
                        self.err_decode = true;
                        self.err_irq = true;
                        self.idle = true;
                    }
                    Err(BusError::Slave) => {
                        self.err_slave = true;
                        self.err_irq = true;
                        self.idle = true;
                    }
                    Ok(()) => {
                        let r_write = if self.keyhole_write {
                            let mut ok = Ok(());
                            for b in &buf {
                                let single = [*b];
                                match ps.mem_write(self.cfg.ps_port, self.da, &single) {
                                    Ok(()) => {}
                                    Err(e) => {
                                        ok = Err(e);
                                        break;
                                    }
                                }
                            }
                            ok
                        } else {
                            ps.mem_write(self.cfg.ps_port, self.da, &buf)
                        };

                        self.record(self.da, len, true, &buf, r_write.is_ok());

                        match r_write {
                            Err(BusError::Decode) => {
                                self.err_decode = true;
                                self.err_irq = true;
                                self.idle = true;
                            }
                            Err(BusError::Slave) => {
                                self.err_slave = true;
                                self.err_irq = true;
                                self.idle = true;
                            }
                            Ok(()) => {
                                self.stats.transfers += 1;
                                self.stats.bytes += len as u64;
                                self.ioc_irq = true;
                                self.idle = true;
                            }
                        }
                    }
                }
            }
            self.pending_simple = false;
        }

        // 2. Scatter-Gather mode transfer
        if self.pending_sg {
            let mut curr = self.curdesc;
            let mut steps = 0;
            const MAX_BD_STEPS: usize = 1024;

            while steps < MAX_BD_STEPS {
                steps += 1;
                let mut bd = [0u8; 32];
                if let Err(e) = ps.mem_read(self.cfg.ps_port, curr, &mut bd) {
                    match e {
                        BusError::Decode => self.err_sg_dec = true,
                        BusError::Slave => self.err_sg_slv = true,
                    }
                    self.err_irq = true;
                    self.idle = true;
                    break;
                }

                let nxtdesc = u32::from_le_bytes(bd[0..4].try_into().unwrap());
                let nxtdesc_msb = u32::from_le_bytes(bd[4..8].try_into().unwrap());
                let buf_src = u32::from_le_bytes(bd[8..12].try_into().unwrap());
                let buf_src_msb = u32::from_le_bytes(bd[12..16].try_into().unwrap());
                let buf_dst = u32::from_le_bytes(bd[16..20].try_into().unwrap());
                let buf_dst_msb = u32::from_le_bytes(bd[20..24].try_into().unwrap());
                let ctrl_len = u32::from_le_bytes(bd[24..28].try_into().unwrap());
                let status = u32::from_le_bytes(bd[28..32].try_into().unwrap());

                // If descriptor is already completed, report error
                if status & 0x8000_0000 != 0 {
                    self.err_sg_int = true;
                    self.err_irq = true;
                    self.idle = true;
                    break;
                }

                let src_addr = u64::from(buf_src) | (u64::from(buf_src_msb) << 32);
                let dst_addr = u64::from(buf_dst) | (u64::from(buf_dst_msb) << 32);
                let len = (ctrl_len & 0x7F_FFFF) as usize;

                let mut transfer_err = false;
                if len > 0 {
                    let mut buf = vec![0u8; len];
                    if let Err(e) = ps.mem_read(self.cfg.ps_port, src_addr, &mut buf) {
                        match e {
                            BusError::Decode => self.err_decode = true,
                            BusError::Slave => self.err_slave = true,
                        }
                        self.err_irq = true;
                        transfer_err = true;
                    } else if let Err(e) = ps.mem_write(self.cfg.ps_port, dst_addr, &buf) {
                        match e {
                            BusError::Decode => self.err_decode = true,
                            BusError::Slave => self.err_slave = true,
                        }
                        self.err_irq = true;
                        transfer_err = true;
                    } else {
                        self.stats.transfers += 1;
                        self.stats.bytes += len as u64;
                    }
                }

                // Write back BD status (bit 31 Completed, bits 22:0 bytes transferred)
                let new_status = if transfer_err {
                    0x8000_0000 | 0x4000_0000 // completed + error
                } else {
                    0x8000_0000 | (len as u32 & 0x7F_FFFF)
                };
                let _ = ps.mem_write(self.cfg.ps_port, curr + 28, &new_status.to_le_bytes());

                self.stats.descriptors += 1;
                self.curdesc = curr;

                if transfer_err {
                    self.idle = true;
                    break;
                }

                if curr == self.taildesc {
                    self.ioc_irq = true;
                    self.idle = true;
                    break;
                }

                let next = u64::from(nxtdesc) | (u64::from(nxtdesc_msb) << 32);
                if next == 0 {
                    self.err_sg_int = true;
                    self.err_irq = true;
                    self.idle = true;
                    break;
                }
                curr = next;
            }

            self.pending_sg = false;
        }
    }

    fn irq(&self) -> bool {
        (self.ioc_irq_en && self.ioc_irq)
            || (self.dly_irq_en && self.dly_irq)
            || (self.err_irq_en && self.err_irq)
    }
}

/// Behavioural register-level model of the Xilinx AXI CDMA 4.1 core.
pub struct AxiCdma {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Client handle to control or inspect the CDMA instance outside register accesses.
#[derive(Clone)]
pub struct CdmaHandle {
    st: Arc<Mutex<State>>,
}

impl AxiCdma {
    /// Creates a new CDMA instance with `cfg` and returns `(AxiCdma, CdmaHandle)`.
    pub fn new(name: &str, cfg: CdmaConfig) -> (AxiCdma, CdmaHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg)));
        (
            AxiCdma {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            CdmaHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl CdmaHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance configuration.
    pub fn config(&self) -> CdmaConfig {
        self.lock().cfg.clone()
    }

    /// Performance and transfer counters.
    pub fn stats(&self) -> CdmaStats {
        self.lock().stats
    }

    /// Removes and returns the master memory access log.
    pub fn take_log(&self) -> Vec<MasterAccess> {
        std::mem::take(&mut self.lock().log)
    }

    /// Status register word `CDMASR`.
    pub fn status(&self) -> u32 {
        self.lock().sr()
    }
}

impl RegisterBlock for AxiCdma {
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
        self.lock().irq()
    }

    fn service(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        let mut s = self.lock();
        s.now = s.now.max(now_ns);
        s.run(ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockMem {
        data: Vec<u8>,
        fail_reads: bool,
    }

    impl MockMem {
        fn new(size: usize) -> Self {
            MockMem {
                data: vec![0; size],
                fail_reads: false,
            }
        }
    }

    impl PsPort for MockMem {
        fn mem_read(
            &mut self,
            _port: PsMasterPort,
            addr: u64,
            buf: &mut [u8],
        ) -> Result<(), BusError> {
            if self.fail_reads {
                return Err(BusError::Slave);
            }
            let start = addr as usize;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(BusError::Decode);
            }
            buf.copy_from_slice(&self.data[start..end]);
            Ok(())
        }

        fn mem_write(
            &mut self,
            _port: PsMasterPort,
            addr: u64,
            buf: &[u8],
        ) -> Result<(), BusError> {
            let start = addr as usize;
            let end = start + buf.len();
            if end > self.data.len() {
                return Err(BusError::Decode);
            }
            self.data[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn irq(&mut self, _irq: u32, _level: bool, _now_ns: u64) -> Result<(), BusError> {
            Ok(())
        }
    }

    #[test]
    fn reset_defaults() {
        let (mut cdma, _) = AxiCdma::new("cdma", CdmaConfig::default());
        assert_eq!(cdma.read32(CDMACR), 0x0001_0000);
        let sr = cdma.read32(CDMASR);
        assert_ne!(sr & SR_IDLE, 0);
        assert_ne!(sr & SR_SGINCLD, 0);
        assert!(!cdma.irq_level());
    }

    #[test]
    fn simple_transfer_and_interrupt() {
        let (mut cdma, handle) = AxiCdma::new("cdma", CdmaConfig::default());
        let mut mem = MockMem::new(0x10000);

        // Fill source memory at 0x1000
        for i in 0..64 {
            mem.data[0x1000 + i] = (i + 1) as u8;
        }

        // Enable IOC interrupt
        cdma.write32(CDMACR, CR_IOC_IRQ_EN, 0xF);

        // Configure simple transfer: SA = 0x1000, DA = 0x2000, BTT = 64
        cdma.write32(SA, 0x1000, 0xF);
        cdma.write32(DA, 0x2000, 0xF);
        cdma.write32(BTT, 64, 0xF);

        // Engine should now be busy
        assert_eq!(cdma.read32(CDMASR) & SR_IDLE, 0);

        // Service the transfer
        cdma.service(100, &mut mem);

        // Engine should now be idle and IOC_Irq set
        let sr = cdma.read32(CDMASR);
        assert_ne!(sr & SR_IDLE, 0);
        assert_ne!(sr & SR_IOC_IRQ, 0);
        assert!(cdma.irq_level());

        // Verify data was copied
        assert_eq!(&mem.data[0x2000..0x2040], &mem.data[0x1000..0x1040]);
        assert_eq!(handle.stats().transfers, 1);
        assert_eq!(handle.stats().bytes, 64);

        // Clear IOC interrupt by writing 1 to status
        cdma.write32(CDMASR, SR_IOC_IRQ, 0xF);
        assert_eq!(cdma.read32(CDMASR) & SR_IOC_IRQ, 0);
        assert!(!cdma.irq_level());
    }

    #[test]
    fn simple_transfer_decode_error() {
        let (mut cdma, _) = AxiCdma::new("cdma", CdmaConfig::default());
        let mut mem = MockMem::new(0x1000); // 4 KiB memory

        cdma.write32(CDMACR, CR_ERR_IRQ_EN, 0xF);
        cdma.write32(SA, 0x2000, 0xF); // outside 4 KiB memory
        cdma.write32(DA, 0x3000, 0xF);
        cdma.write32(BTT, 64, 0xF);

        cdma.service(100, &mut mem);

        let sr = cdma.read32(CDMASR);
        assert_ne!(sr & SR_IDLE, 0);
        assert_ne!(sr & SR_ERR_DECODE, 0);
        assert_ne!(sr & SR_ERR_IRQ, 0);
        assert!(cdma.irq_level());
    }

    #[test]
    fn scatter_gather_transfer() {
        let (mut cdma, handle) = AxiCdma::new("cdma", CdmaConfig::default());
        let mut mem = MockMem::new(0x10000);

        // Source buffer 1 at 0x1000, dest 1 at 0x2000, len 32
        for i in 0..32 {
            mem.data[0x1000 + i] = 0xAA;
        }
        // Source buffer 2 at 0x1100, dest 2 at 0x2100, len 32
        for i in 0..32 {
            mem.data[0x1100 + i] = 0xBB;
        }

        // BD 1 at 0x0100 -> next 0x0140
        let bd1_addr = 0x0100;
        let bd2_addr = 0x0140;

        // BD1: next = bd2_addr, src = 0x1000, dst = 0x2000, len = 32, status = 0
        mem.data[bd1_addr..bd1_addr + 4].copy_from_slice(&(bd2_addr as u32).to_le_bytes());
        mem.data[bd1_addr + 8..bd1_addr + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        mem.data[bd1_addr + 16..bd1_addr + 20].copy_from_slice(&0x2000u32.to_le_bytes());
        mem.data[bd1_addr + 24..bd1_addr + 28].copy_from_slice(&32u32.to_le_bytes());

        // BD2: next = 0, src = 0x1100, dst = 0x2100, len = 32, status = 0
        mem.data[bd2_addr + 8..bd2_addr + 12].copy_from_slice(&0x1100u32.to_le_bytes());
        mem.data[bd2_addr + 16..bd2_addr + 20].copy_from_slice(&0x2100u32.to_le_bytes());
        mem.data[bd2_addr + 24..bd2_addr + 28].copy_from_slice(&32u32.to_le_bytes());

        // Enable SG mode and IOC interrupt
        cdma.write32(CDMACR, CR_SGMODE | CR_IOC_IRQ_EN, 0xF);
        cdma.write32(CURDESC_PNTR, bd1_addr as u32, 0xF);
        cdma.write32(TAILDESC_PNTR, bd2_addr as u32, 0xF);

        cdma.service(200, &mut mem);

        let sr = cdma.read32(CDMASR);
        assert_ne!(sr & SR_IDLE, 0);
        assert_ne!(sr & SR_IOC_IRQ, 0);
        assert!(cdma.irq_level());

        // Both buffers copied
        assert_eq!(&mem.data[0x2000..0x2020], &[0xAA; 32]);
        assert_eq!(&mem.data[0x2100..0x2120], &[0xBB; 32]);

        // BD status completed bit (bit 31)
        let s1 = u32::from_le_bytes(mem.data[bd1_addr + 28..bd1_addr + 32].try_into().unwrap());
        let s2 = u32::from_le_bytes(mem.data[bd2_addr + 28..bd2_addr + 32].try_into().unwrap());
        assert_ne!(s1 & 0x8000_0000, 0);
        assert_ne!(s2 & 0x8000_0000, 0);

        assert_eq!(handle.stats().descriptors, 2);
        assert_eq!(handle.stats().transfers, 2);
        assert_eq!(handle.stats().bytes, 64);
    }
}
