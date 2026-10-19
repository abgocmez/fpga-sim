//! AXI Video Direct Memory Access (`xilinx.com:ip:axi_vdma:6.3`, Xilinx PG020), register level:
//! MM2S (frame read) and S2MM (frame write) channels, multi-frame buffering, 2D stride/frame
//! addressing, and stream loopback.
//!
//! Register map (offsets from the base address; from PG020):
//!
//! | offset | register | description |
//! |---|---|---|
//! | `0x00` | `MM2S_VDMACR` | MM2S VDMA Control Register |
//! | `0x04` | `MM2S_VDMASR` | MM2S VDMA Status Register |
//! | `0x14` | `MM2S_REG_INDEX` | MM2S Register Index |
//! | `0x28` | `PARK_PTR_REG` | Park Pointer Register (WrFrmPtr bits 4:0, RdFrmPtr bits 12:8) |
//! | `0x2C` | `VDMA_VERSION` | VDMA Version Register (`0x0603_0000`) |
//! | `0x30` | `S2MM_VDMACR` | S2MM VDMA Control Register |
//! | `0x34` | `S2MM_VDMASR` | S2MM VDMA Status Register |
//! | `0x3C` | `S2MM_IRQ_MASK` | S2MM Error Interrupt Mask Register |
//! | `0x44` | `S2MM_REG_INDEX` | S2MM Register Index |
//! | `0x50` | `MM2S_VSIZE` | MM2S Vertical Size (lines; writing starts transfer) |
//! | `0x54` | `MM2S_HSIZE` | MM2S Horizontal Size (bytes per line) |
//! | `0x58` | `MM2S_FRMDLY_STRIDE` | MM2S Frame Delay & Stride (bytes) |
//! | `0x5C`..`0x98` | `MM2S_START_ADDRESS[1..16]` | MM2S Frame Buffer Start Addresses (up to 32) |
//! | `0xA0` | `S2MM_VSIZE` | S2MM Vertical Size (lines; writing starts transfer) |
//! | `0xA4` | `S2MM_HSIZE` | S2MM Horizontal Size (bytes per line) |
//! | `0xA8` | `S2MM_FRMDLY_STRIDE` | S2MM Frame Delay & Stride (bytes) |
//! | `0xAC`..`0xE8` | `S2MM_START_ADDRESS[1..16]` | S2MM Frame Buffer Start Addresses (up to 32) |
//!
//! `VDMACR`: bit 0 `RS` (run/stop), bit 1 `Circular_Park` (1 = circular, 0 = park), bit 2 `Reset`
//! (self-clearing, resets the channel), bit 3 `GenlockEn`, bit 6 `FrameCntEn`, bit 12 `IOC_IrqEn`,
//! bit 13 `Dly_IrqEn`, bit 14 `Err_IrqEn`, bits 23:16 `FrameCount` interrupt threshold.
//!
//! `VDMASR`: bit 0 `Halted`, bit 1 `Idle`, bit 4 `DMAIntErr`, bit 5 `DMASlvErr`, bit 6 `DMADecErr`,
//! bit 7 `FSize_Err`, bit 12 `IOC_Irq` (write 1 to clear), bit 13 `Dly_Irq` (W1C), bit 14 `Err_Irq`
//! (W1C), bits 28:24 `FrmPntr` (active/last frame index).
//!
//! `VERIFY:` items (not checked against hardware or Xilinx simulation):
//! * `VDMA_VERSION` reads `0x0603_0000` (PG020 v6.3);
//! * `MM2S_VDMASR` and `S2MM_VDMASR` reset to `0x0001_0001` (Halted = 1);
//! * Writing `MM2S_VSIZE` triggers frame transfer if `RS = 1`;
//! * In park mode, the buffer selected in `PARK_PTR_REG` is used for every frame;
//! * In circular mode, the buffer pointer advances modulo `C_NUM_FSTORES`;
//! * Two interrupt lines: output 0 is `mm2s_introut`, output 1 is `s2mm_introut`.

use crate::dma::parse_ps_port;
use crate::params::{Params, flag, number, ranged};
use pl_api::{BusError, MasterAccess, PsMasterPort, PsPort};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `MM2S_VDMACR`: MM2S Control Register.
pub const MM2S_VDMACR: u64 = 0x00;
/// `MM2S_VDMASR`: MM2S Status Register.
pub const MM2S_VDMASR: u64 = 0x04;
/// `MM2S_REG_INDEX`: MM2S Register Index.
pub const MM2S_REG_INDEX: u64 = 0x14;
/// `PARK_PTR_REG`: Park Pointer Register.
pub const PARK_PTR_REG: u64 = 0x28;
/// `VDMA_VERSION`: Version Register.
pub const VDMA_VERSION: u64 = 0x2C;
/// `S2MM_VDMACR`: S2MM Control Register.
pub const S2MM_VDMACR: u64 = 0x30;
/// `S2MM_VDMASR`: S2MM Status Register.
pub const S2MM_VDMASR: u64 = 0x34;
/// `S2MM_IRQ_MASK`: S2MM Error Interrupt Mask Register.
pub const S2MM_IRQ_MASK: u64 = 0x3C;
/// `S2MM_REG_INDEX`: S2MM Register Index.
pub const S2MM_REG_INDEX: u64 = 0x44;
/// `MM2S_VSIZE`: MM2S Vertical Size (writing starts transfer).
pub const MM2S_VSIZE: u64 = 0x50;
/// `MM2S_HSIZE`: MM2S Horizontal Size (bytes per line).
pub const MM2S_HSIZE: u64 = 0x54;
/// `MM2S_FRMDLY_STRIDE`: MM2S Frame Delay & Stride.
pub const MM2S_FRMDLY_STRIDE: u64 = 0x58;
/// `MM2S_START_ADDRESS_BASE`: MM2S Frame Buffer Start Address 0.
pub const MM2S_START_ADDRESS_BASE: u64 = 0x5C;
/// `S2MM_VSIZE`: S2MM Vertical Size (writing starts transfer).
pub const S2MM_VSIZE: u64 = 0xA0;
/// `S2MM_HSIZE`: S2MM Horizontal Size (bytes per line).
pub const S2MM_HSIZE: u64 = 0xA4;
/// `S2MM_FRMDLY_STRIDE`: S2MM Frame Delay & Stride.
pub const S2MM_FRMDLY_STRIDE: u64 = 0xA8;
/// `S2MM_START_ADDRESS_BASE`: S2MM Frame Buffer Start Address 0.
pub const S2MM_START_ADDRESS_BASE: u64 = 0xAC;

/// Core version word (v6.3).
pub const VERSION_6_3: u32 = 0x0603_0000;

/// `VDMACR` Run/Stop bit.
pub const CR_RS: u32 = 1 << 0;
/// `VDMACR` Circular / Park mode bit (1 = circular, 0 = park).
pub const CR_CIRCULAR_PARK: u32 = 1 << 1;
/// `VDMACR` Reset bit.
pub const CR_RESET: u32 = 1 << 2;
/// `VDMACR` Genlock Enable bit.
pub const CR_GENLOCK_EN: u32 = 1 << 3;
/// `VDMACR` Frame Count Enable bit.
pub const CR_FRAME_CNT_EN: u32 = 1 << 6;
/// `VDMACR` IOC Interrupt Enable bit.
pub const CR_IOC_IRQ_EN: u32 = 1 << 12;
/// `VDMACR` Delay Interrupt Enable bit.
pub const CR_DLY_IRQ_EN: u32 = 1 << 13;
/// `VDMACR` Error Interrupt Enable bit.
pub const CR_ERR_IRQ_EN: u32 = 1 << 14;

/// `VDMASR` Halted status bit.
pub const SR_HALTED: u32 = 1 << 0;
/// `VDMASR` Idle status bit.
pub const SR_IDLE: u32 = 1 << 1;
/// `VDMASR` DMA Internal Error bit.
pub const SR_DMA_INT_ERR: u32 = 1 << 4;
/// `VDMASR` DMA Slave Error bit.
pub const SR_DMA_SLV_ERR: u32 = 1 << 5;
/// `VDMASR` DMA Decode Error bit.
pub const SR_DMA_DEC_ERR: u32 = 1 << 6;
/// `VDMASR` Frame Size Error bit.
pub const SR_FSIZE_ERR: u32 = 1 << 7;
/// `VDMASR` IOC Interrupt bit (write 1 to clear).
pub const SR_IOC_IRQ: u32 = 1 << 12;
/// `VDMASR` Delay Interrupt bit (write 1 to clear).
pub const SR_DLY_IRQ: u32 = 1 << 13;
/// `VDMASR` Error Interrupt bit (write 1 to clear).
pub const SR_ERR_IRQ: u32 = 1 << 14;

/// Static configuration of an AXI VDMA instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VdmaConfig {
    /// Whether MM2S (read) channel is present.
    pub mm2s: bool,
    /// Whether S2MM (write) channel is present.
    pub s2mm: bool,
    /// Number of frame stores (1..=32).
    pub num_fstores: usize,
    /// Memory port the VDMA uses for PS master accesses.
    pub ps_port: PsMasterPort,
    /// Whether stream loopback is active (MM2S frames loop into S2MM).
    pub loopback: bool,
    /// Address window size in bytes.
    pub window: u64,
}

impl Default for VdmaConfig {
    fn default() -> Self {
        VdmaConfig {
            mm2s: true,
            s2mm: true,
            num_fstores: 3,
            ps_port: PsMasterPort::SAxiHp0,
            loopback: true,
            window: 0x1_0000,
        }
    }
}

impl VdmaConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<VdmaConfig, String> {
        let fstores = ranged(p, "C_NUM_FSTORES", 3, 1, 32)? as usize;
        let port = match p.get("PS_PORT") {
            Some(v) => parse_ps_port(v)?,
            None => PsMasterPort::SAxiHp0,
        };
        Ok(VdmaConfig {
            mm2s: flag(p, "C_INCLUDE_MM2S", true)?,
            s2mm: flag(p, "C_INCLUDE_S2MM", true)?,
            num_fstores: fstores,
            ps_port: port,
            loopback: number(p, "STREAM_LOOPBACK", 1)? != 0,
            window,
        })
    }
}

/// Dynamic state of one VDMA channel (MM2S or S2MM).
#[derive(Clone, Debug)]
struct ChannelState {
    rs: bool,
    circular: bool,
    genlock_en: bool,
    frame_cnt_en: bool,
    ioc_irq_en: bool,
    dly_irq_en: bool,
    err_irq_en: bool,
    irq_threshold: u8,
    irq_delay: u8,

    halted_err: bool,
    int_err: bool,
    slv_err: bool,
    dec_err: bool,
    fsize_err: bool,

    ioc: bool,
    dly: bool,
    err: bool,

    vsize: u32,
    hsize: u32,
    stride: u32,
    start_addrs: Vec<u64>,
    cur_ptr: usize,
    last_ptr: usize,
    pending_transfer: bool,
}

impl ChannelState {
    fn new(num_fstores: usize) -> Self {
        ChannelState {
            rs: false,
            circular: false,
            genlock_en: false,
            frame_cnt_en: false,
            ioc_irq_en: false,
            dly_irq_en: false,
            err_irq_en: false,
            irq_threshold: 1,
            irq_delay: 0,
            halted_err: false,
            int_err: false,
            slv_err: false,
            dec_err: false,
            fsize_err: false,
            ioc: false,
            dly: false,
            err: false,
            vsize: 0,
            hsize: 0,
            stride: 0,
            start_addrs: vec![0; num_fstores],
            cur_ptr: 0,
            last_ptr: 0,
            pending_transfer: false,
        }
    }

    fn reset(&mut self) {
        self.rs = false;
        self.circular = false;
        self.genlock_en = false;
        self.frame_cnt_en = false;
        self.ioc_irq_en = false;
        self.dly_irq_en = false;
        self.err_irq_en = false;
        self.irq_threshold = 1;
        self.irq_delay = 0;
        self.halted_err = false;
        self.int_err = false;
        self.slv_err = false;
        self.dec_err = false;
        self.fsize_err = false;
        self.ioc = false;
        self.dly = false;
        self.err = false;
        self.vsize = 0;
        self.hsize = 0;
        self.stride = 0;
        self.cur_ptr = 0;
        self.last_ptr = 0;
        self.pending_transfer = false;
    }

    fn cr(&self) -> u32 {
        let mut v = 0;
        if self.rs {
            v |= CR_RS;
        }
        if self.circular {
            v |= CR_CIRCULAR_PARK;
        }
        if self.genlock_en {
            v |= CR_GENLOCK_EN;
        }
        if self.frame_cnt_en {
            v |= CR_FRAME_CNT_EN;
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
        self.rs = val & CR_RS != 0;
        self.circular = val & CR_CIRCULAR_PARK != 0;
        self.genlock_en = val & CR_GENLOCK_EN != 0;
        self.frame_cnt_en = val & CR_FRAME_CNT_EN != 0;
        self.ioc_irq_en = val & CR_IOC_IRQ_EN != 0;
        self.dly_irq_en = val & CR_DLY_IRQ_EN != 0;
        self.err_irq_en = val & CR_ERR_IRQ_EN != 0;
        let thr = (val >> 16) & 0xFF;
        self.irq_threshold = if thr == 0 { 1 } else { thr as u8 };
        self.irq_delay = (val >> 24) as u8;
    }

    fn sr(&self) -> u32 {
        let mut v = 0;
        if !self.rs || self.halted_err {
            v |= SR_HALTED;
        }
        if self.rs && !self.pending_transfer {
            v |= SR_IDLE;
        }
        if self.int_err {
            v |= SR_DMA_INT_ERR;
        }
        if self.slv_err {
            v |= SR_DMA_SLV_ERR;
        }
        if self.dec_err {
            v |= SR_DMA_DEC_ERR;
        }
        if self.fsize_err {
            v |= SR_FSIZE_ERR;
        }
        if self.ioc {
            v |= SR_IOC_IRQ;
        }
        if self.dly {
            v |= SR_DLY_IRQ;
        }
        if self.err {
            v |= SR_ERR_IRQ;
        }
        v |= ((self.last_ptr as u32) & 0x1F) << 24;
        v
    }

    fn write_sr(&mut self, val: u32) {
        // W1C for interrupt and error bits
        if val & SR_IOC_IRQ != 0 {
            self.ioc = false;
        }
        if val & SR_DLY_IRQ != 0 {
            self.dly = false;
        }
        if val & SR_ERR_IRQ != 0 {
            self.err = false;
            self.int_err = false;
            self.slv_err = false;
            self.dec_err = false;
            self.fsize_err = false;
            self.halted_err = false;
        }
    }

    fn irq(&self) -> bool {
        (self.ioc_irq_en && self.ioc)
            || (self.dly_irq_en && self.dly)
            || (self.err_irq_en && self.err)
    }

    fn fail(&mut self, err: BusError) {
        match err {
            BusError::Decode => self.dec_err = true,
            BusError::Slave => self.slv_err = true,
        }
        self.err = true;
        self.halted_err = true;
        self.pending_transfer = false;
    }
}

/// Statistics collected by the VDMA engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VdmaStats {
    /// Number of video frames transmitted on MM2S.
    pub mm2s_frames: u64,
    /// Total bytes transmitted on MM2S.
    pub mm2s_bytes: u64,
    /// Number of video frames received on S2MM.
    pub s2mm_frames: u64,
    /// Total bytes received on S2MM.
    pub s2mm_bytes: u64,
}

/// Internal state of the VDMA instance.
struct State {
    cfg: VdmaConfig,
    now: u64,
    park_wr_ptr: u8,
    park_rd_ptr: u8,
    mm2s_reg_idx: u32,
    s2mm_reg_idx: u32,
    mm2s: ChannelState,
    s2mm: ChannelState,
    to_s2mm: VecDeque<Vec<u8>>,
    from_mm2s: VecDeque<Vec<u8>>,
    log: Vec<MasterAccess>,
    stats: VdmaStats,
}

impl State {
    fn power_on(cfg: VdmaConfig) -> Self {
        let fstores = cfg.num_fstores;
        State {
            cfg,
            now: 0,
            park_wr_ptr: 0,
            park_rd_ptr: 0,
            mm2s_reg_idx: 0,
            s2mm_reg_idx: 0,
            mm2s: ChannelState::new(fstores),
            s2mm: ChannelState::new(fstores),
            to_s2mm: VecDeque::new(),
            from_mm2s: VecDeque::new(),
            log: Vec::new(),
            stats: VdmaStats::default(),
        }
    }

    fn reset(&mut self) {
        self.mm2s.reset();
        self.s2mm.reset();
        self.park_wr_ptr = 0;
        self.park_rd_ptr = 0;
        self.mm2s_reg_idx = 0;
        self.s2mm_reg_idx = 0;
        self.to_s2mm.clear();
        self.from_mm2s.clear();
    }

    fn read_reg(&self, offset: u64) -> u32 {
        match offset {
            MM2S_VDMACR => {
                if self.cfg.mm2s {
                    self.mm2s.cr()
                } else {
                    0
                }
            }
            MM2S_VDMASR => {
                if self.cfg.mm2s {
                    self.mm2s.sr()
                } else {
                    0
                }
            }
            MM2S_REG_INDEX => self.mm2s_reg_idx,
            PARK_PTR_REG => (self.park_wr_ptr as u32) | ((self.park_rd_ptr as u32) << 8),
            VDMA_VERSION => VERSION_6_3,
            S2MM_VDMACR => {
                if self.cfg.s2mm {
                    self.s2mm.cr()
                } else {
                    0
                }
            }
            S2MM_VDMASR => {
                if self.cfg.s2mm {
                    self.s2mm.sr()
                } else {
                    0
                }
            }
            S2MM_REG_INDEX => self.s2mm_reg_idx,
            MM2S_VSIZE => self.mm2s.vsize,
            MM2S_HSIZE => self.mm2s.hsize,
            MM2S_FRMDLY_STRIDE => self.mm2s.stride,
            S2MM_VSIZE => self.s2mm.vsize,
            S2MM_HSIZE => self.s2mm.hsize,
            S2MM_FRMDLY_STRIDE => self.s2mm.stride,
            o if (MM2S_START_ADDRESS_BASE..=0x98).contains(&o)
                && (o - MM2S_START_ADDRESS_BASE) % 4 == 0 =>
            {
                let bank = (self.mm2s_reg_idx & 1) as usize;
                let idx = bank * 16 + ((o - MM2S_START_ADDRESS_BASE) / 4) as usize;
                if idx < self.mm2s.start_addrs.len() {
                    self.mm2s.start_addrs[idx] as u32
                } else {
                    0
                }
            }
            o if (S2MM_START_ADDRESS_BASE..=0xE8).contains(&o)
                && (o - S2MM_START_ADDRESS_BASE) % 4 == 0 =>
            {
                let bank = (self.s2mm_reg_idx & 1) as usize;
                let idx = bank * 16 + ((o - S2MM_START_ADDRESS_BASE) / 4) as usize;
                if idx < self.s2mm.start_addrs.len() {
                    self.s2mm.start_addrs[idx] as u32
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, _strobe: u8) {
        match offset {
            MM2S_VDMACR => {
                if self.cfg.mm2s {
                    self.mm2s.write_cr(value);
                }
            }
            MM2S_VDMASR => {
                if self.cfg.mm2s {
                    self.mm2s.write_sr(value);
                }
            }
            MM2S_REG_INDEX => self.mm2s_reg_idx = value,
            PARK_PTR_REG => {
                self.park_wr_ptr = (value & 0x1F) as u8;
                self.park_rd_ptr = ((value >> 8) & 0x1F) as u8;
            }
            S2MM_VDMACR => {
                if self.cfg.s2mm {
                    self.s2mm.write_cr(value);
                }
            }
            S2MM_VDMASR => {
                if self.cfg.s2mm {
                    self.s2mm.write_sr(value);
                }
            }
            S2MM_REG_INDEX => self.s2mm_reg_idx = value,
            MM2S_VSIZE => {
                if self.cfg.mm2s {
                    self.mm2s.vsize = value & 0x1FFF;
                    if self.mm2s.rs && !self.mm2s.halted_err {
                        self.mm2s.pending_transfer = true;
                    }
                }
            }
            MM2S_HSIZE => {
                if self.cfg.mm2s {
                    self.mm2s.hsize = value & 0xFFFF;
                }
            }
            MM2S_FRMDLY_STRIDE => {
                if self.cfg.mm2s {
                    self.mm2s.stride = value & 0xFFFF;
                }
            }
            S2MM_VSIZE => {
                if self.cfg.s2mm {
                    self.s2mm.vsize = value & 0x1FFF;
                    if self.s2mm.rs && !self.s2mm.halted_err {
                        self.s2mm.pending_transfer = true;
                    }
                }
            }
            S2MM_HSIZE => {
                if self.cfg.s2mm {
                    self.s2mm.hsize = value & 0xFFFF;
                }
            }
            S2MM_FRMDLY_STRIDE => {
                if self.cfg.s2mm {
                    self.s2mm.stride = value & 0xFFFF;
                }
            }
            o if (MM2S_START_ADDRESS_BASE..=0x98).contains(&o)
                && (o - MM2S_START_ADDRESS_BASE) % 4 == 0 =>
            {
                let bank = (self.mm2s_reg_idx & 1) as usize;
                let idx = bank * 16 + ((o - MM2S_START_ADDRESS_BASE) / 4) as usize;
                if idx < self.mm2s.start_addrs.len() {
                    self.mm2s.start_addrs[idx] = u64::from(value);
                }
            }
            o if (S2MM_START_ADDRESS_BASE..=0xE8).contains(&o)
                && (o - S2MM_START_ADDRESS_BASE) % 4 == 0 =>
            {
                let bank = (self.s2mm_reg_idx & 1) as usize;
                let idx = bank * 16 + ((o - S2MM_START_ADDRESS_BASE) / 4) as usize;
                if idx < self.s2mm.start_addrs.len() {
                    self.s2mm.start_addrs[idx] = u64::from(value);
                }
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
        let num_fstores = self.cfg.num_fstores;

        // 1. MM2S channel
        if self.cfg.mm2s && self.mm2s.rs && !self.mm2s.halted_err && self.mm2s.pending_transfer {
            let fidx = if self.mm2s.circular {
                self.mm2s.cur_ptr
            } else {
                (self.park_rd_ptr as usize) % num_fstores
            };
            let base_addr = self.mm2s.start_addrs[fidx];
            let vsize = self.mm2s.vsize;
            let hsize = self.mm2s.hsize;
            let stride = self.mm2s.stride.max(hsize);

            if vsize == 0 || hsize == 0 {
                self.mm2s.fsize_err = true;
                self.mm2s.err = true;
                self.mm2s.halted_err = true;
                self.mm2s.pending_transfer = false;
            } else {
                let mut frame = Vec::with_capacity((vsize * hsize) as usize);
                let mut read_err = None;
                for y in 0..vsize {
                    let line_addr = base_addr + (y as u64) * (stride as u64);
                    let mut line_buf = vec![0u8; hsize as usize];
                    let r = ps.mem_read(self.cfg.ps_port, line_addr, &mut line_buf);
                    self.record(line_addr, hsize as usize, false, &line_buf, r.is_ok());
                    match r {
                        Ok(()) => frame.extend_from_slice(&line_buf),
                        Err(e) => {
                            read_err = Some(e);
                            break;
                        }
                    }
                }

                if let Some(e) = read_err {
                    self.mm2s.fail(e);
                } else {
                    self.stats.mm2s_frames += 1;
                    self.stats.mm2s_bytes += frame.len() as u64;
                    if self.cfg.loopback {
                        self.to_s2mm.push_back(frame);
                    } else {
                        self.from_mm2s.push_back(frame);
                    }
                    self.mm2s.last_ptr = fidx;
                    if self.mm2s.circular {
                        self.mm2s.cur_ptr = (self.mm2s.cur_ptr + 1) % num_fstores;
                    }
                    self.mm2s.ioc = true;
                }
                self.mm2s.pending_transfer = false;
            }
        }

        // 2. S2MM channel
        if self.cfg.s2mm && self.s2mm.rs && !self.s2mm.halted_err && self.s2mm.vsize > 0 {
            while let Some(frame) = self.to_s2mm.pop_front() {
                let fidx = if self.s2mm.circular {
                    self.s2mm.cur_ptr
                } else {
                    (self.park_wr_ptr as usize) % num_fstores
                };
                let base_addr = self.s2mm.start_addrs[fidx];
                let vsize = self.s2mm.vsize;
                let hsize = self.s2mm.hsize;
                let stride = self.s2mm.stride.max(hsize);

                let mut write_err = None;
                for y in 0..vsize {
                    let line_addr = base_addr + (y as u64) * (stride as u64);
                    let start = (y * hsize) as usize;
                    let end = start + (hsize as usize);
                    let slice = if end <= frame.len() {
                        &frame[start..end]
                    } else if start < frame.len() {
                        &frame[start..]
                    } else {
                        &[]
                    };
                    let r = ps.mem_write(self.cfg.ps_port, line_addr, slice);
                    self.record(line_addr, slice.len(), true, slice, r.is_ok());
                    if let Err(e) = r {
                        write_err = Some(e);
                        break;
                    }
                }

                if let Some(e) = write_err {
                    self.s2mm.fail(e);
                    break;
                } else {
                    self.stats.s2mm_frames += 1;
                    self.stats.s2mm_bytes += frame.len() as u64;
                    self.s2mm.last_ptr = fidx;
                    if self.s2mm.circular {
                        self.s2mm.cur_ptr = (self.s2mm.cur_ptr + 1) % num_fstores;
                    }
                    self.s2mm.ioc = true;
                }
                self.s2mm.pending_transfer = false;
            }
        }
    }
}

/// Behavioural register-level model of the Xilinx AXI VDMA 6.3 core.
pub struct AxiVdma {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Client handle to control or inspect the VDMA instance outside register accesses.
#[derive(Clone)]
pub struct VdmaHandle {
    st: Arc<Mutex<State>>,
}

impl AxiVdma {
    /// Creates a new VDMA instance with `cfg` and returns `(AxiVdma, VdmaHandle)`.
    pub fn new(name: &str, cfg: VdmaConfig) -> (AxiVdma, VdmaHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg)));
        (
            AxiVdma {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            VdmaHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl VdmaHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance configuration.
    pub fn config(&self) -> VdmaConfig {
        self.lock().cfg.clone()
    }

    /// Drains and returns all frames transmitted by MM2S (when loopback is disabled).
    pub fn take_mm2s(&self) -> Vec<Vec<u8>> {
        self.lock().from_mm2s.drain(..).collect()
    }

    /// Pushes an incoming video frame into the S2MM stream.
    pub fn push_s2mm(&self, frame: Vec<u8>) {
        if !frame.is_empty() {
            self.lock().to_s2mm.push_back(frame);
        }
    }

    /// Removes and returns the master memory access log.
    pub fn take_log(&self) -> Vec<MasterAccess> {
        std::mem::take(&mut self.lock().log)
    }

    /// Performance and transfer counters.
    pub fn stats(&self) -> VdmaStats {
        self.lock().stats
    }

    /// Status register words `(MM2S_VDMASR, S2MM_VDMASR)`.
    pub fn status(&self) -> (u32, u32) {
        let s = self.lock();
        (s.mm2s.sr(), s.s2mm.sr())
    }
}

impl RegisterBlock for AxiVdma {
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
        let s = self.lock();
        match output {
            0 => s.cfg.mm2s && s.mm2s.irq(),
            1 => s.cfg.s2mm && s.s2mm.irq(),
            _ => false,
        }
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
                return Err(BusError::Decode);
            }
            let a = addr as usize;
            if a + buf.len() <= self.data.len() {
                buf.copy_from_slice(&self.data[a..a + buf.len()]);
                Ok(())
            } else {
                Err(BusError::Decode)
            }
        }

        fn mem_write(
            &mut self,
            _port: PsMasterPort,
            addr: u64,
            buf: &[u8],
        ) -> Result<(), BusError> {
            let a = addr as usize;
            if a + buf.len() <= self.data.len() {
                self.data[a..a + buf.len()].copy_from_slice(buf);
                Ok(())
            } else {
                Err(BusError::Decode)
            }
        }

        fn irq(&mut self, _irq: u32, _level: bool, _now_ns: u64) -> Result<(), BusError> {
            Ok(())
        }
    }

    #[test]
    fn reset_and_version_registers() {
        let (mut vdma, _) = AxiVdma::new("vdma0", VdmaConfig::default());
        assert_eq!(vdma.read32(VDMA_VERSION), VERSION_6_3);
        assert_eq!(vdma.read32(MM2S_VDMASR), SR_HALTED);
        assert_eq!(vdma.read32(S2MM_VDMASR), SR_HALTED);
    }

    #[test]
    fn loopback_frame_transfer_in_park_mode() {
        let (mut vdma, handle) = AxiVdma::new("vdma0", VdmaConfig::default());
        let mut mem = MockMem::new(0x4000);

        // Fill source buffer at 0x1000 with pattern
        for (i, b) in mem.data[0x1000..0x1040].iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(0x42);
        }

        // Configure MM2S: buffer at 0x1000, 4 lines of 16 bytes, stride 16
        vdma.write32(MM2S_START_ADDRESS_BASE, 0x1000, 0xF);
        vdma.write32(MM2S_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(MM2S_HSIZE, 16, 0xF);
        vdma.write32(MM2S_VDMACR, CR_RS | CR_IOC_IRQ_EN, 0xF); // Park mode (bit 1 = 0)

        // Configure S2MM: buffer at 0x2000, 4 lines of 16 bytes, stride 16
        vdma.write32(S2MM_START_ADDRESS_BASE, 0x2000, 0xF);
        vdma.write32(S2MM_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(S2MM_HSIZE, 16, 0xF);
        vdma.write32(S2MM_VDMACR, CR_RS | CR_IOC_IRQ_EN, 0xF); // Park mode
        vdma.write32(S2MM_VSIZE, 4, 0xF);

        // Start MM2S by writing VSIZE
        vdma.write32(MM2S_VSIZE, 4, 0xF);

        // Service bus operations
        vdma.service(100, &mut mem);

        // Destination buffer at 0x2000 must match 0x1000
        assert_eq!(&mem.data[0x2000..0x2040], &mem.data[0x1000..0x1040]);

        // Interrupts asserted
        assert!(vdma.irq_output(0), "MM2S interrupt must be active");
        assert!(vdma.irq_output(1), "S2MM interrupt must be active");

        let stats = handle.stats();
        assert_eq!(stats.mm2s_frames, 1);
        assert_eq!(stats.mm2s_bytes, 64);
        assert_eq!(stats.s2mm_frames, 1);
        assert_eq!(stats.s2mm_bytes, 64);
    }

    #[test]
    fn circular_mode_advances_buffers() {
        let (mut vdma, _) = AxiVdma::new(
            "vdma0",
            VdmaConfig {
                num_fstores: 3,
                ..Default::default()
            },
        );
        let mut mem = MockMem::new(0x8000);

        // Set 3 MM2S buffers
        vdma.write32(MM2S_START_ADDRESS_BASE, 0x1000, 0xF);
        vdma.write32(MM2S_START_ADDRESS_BASE + 4, 0x2000, 0xF);
        vdma.write32(MM2S_START_ADDRESS_BASE + 8, 0x3000, 0xF);
        vdma.write32(MM2S_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(MM2S_HSIZE, 16, 0xF);
        vdma.write32(MM2S_VDMACR, CR_RS | CR_CIRCULAR_PARK, 0xF);

        // Frame 0
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(100, &mut mem);
        let sr = vdma.read32(MM2S_VDMASR);
        assert_eq!((sr >> 24) & 0x1F, 0);

        // Frame 1
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(200, &mut mem);
        let sr = vdma.read32(MM2S_VDMASR);
        assert_eq!((sr >> 24) & 0x1F, 1);

        // Frame 2
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(300, &mut mem);
        let sr = vdma.read32(MM2S_VDMASR);
        assert_eq!((sr >> 24) & 0x1F, 2);

        // Frame 3 wraps to 0
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(400, &mut mem);
        let sr = vdma.read32(MM2S_VDMASR);
        assert_eq!((sr >> 24) & 0x1F, 0);
    }

    #[test]
    fn irq_and_w1c_clearing() {
        let (mut vdma, _) = AxiVdma::new("vdma0", VdmaConfig::default());
        let mut mem = MockMem::new(0x4000);

        vdma.write32(MM2S_START_ADDRESS_BASE, 0x1000, 0xF);
        vdma.write32(MM2S_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(MM2S_HSIZE, 16, 0xF);
        vdma.write32(MM2S_VDMACR, CR_RS | CR_IOC_IRQ_EN, 0xF);
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(100, &mut mem);

        assert!(vdma.irq_output(0));
        assert_ne!(vdma.read32(MM2S_VDMASR) & SR_IOC_IRQ, 0);

        // Clear IOC IRQ via W1C
        vdma.write32(MM2S_VDMASR, SR_IOC_IRQ, 0xF);
        assert_eq!(vdma.read32(MM2S_VDMASR) & SR_IOC_IRQ, 0);
        assert!(!vdma.irq_output(0));
    }

    #[test]
    fn bus_error_halts_channel() {
        let (mut vdma, _) = AxiVdma::new("vdma0", VdmaConfig::default());
        let mut mem = MockMem::new(0x4000);
        mem.fail_reads = true;

        vdma.write32(MM2S_START_ADDRESS_BASE, 0x1000, 0xF);
        vdma.write32(MM2S_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(MM2S_HSIZE, 16, 0xF);
        vdma.write32(MM2S_VDMACR, CR_RS | CR_ERR_IRQ_EN, 0xF);
        vdma.write32(MM2S_VSIZE, 2, 0xF);
        vdma.service(100, &mut mem);

        let sr = vdma.read32(MM2S_VDMASR);
        assert_ne!(sr & SR_HALTED, 0, "Channel must halt on bus error");
        assert_ne!(sr & SR_DMA_DEC_ERR, 0, "Dec error must be set");
        assert_ne!(sr & SR_ERR_IRQ, 0, "Error IRQ must be active");
        assert!(vdma.irq_output(0), "Interrupt output 0 must fire on error");
    }

    #[test]
    fn non_loopback_stream_fifo() {
        let (mut vdma, handle) = AxiVdma::new(
            "vdma0",
            VdmaConfig {
                loopback: false,
                ..Default::default()
            },
        );
        let mut mem = MockMem::new(0x4000);

        // MM2S transmit
        vdma.write32(MM2S_START_ADDRESS_BASE, 0x1000, 0xF);
        vdma.write32(MM2S_FRMDLY_STRIDE, 16, 0xF);
        vdma.write32(MM2S_HSIZE, 16, 0xF);
        vdma.write32(MM2S_VDMACR, CR_RS, 0xF);
        vdma.write32(MM2S_VSIZE, 1, 0xF);
        vdma.service(100, &mut mem);

        let frames = handle.take_mm2s();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 16);

        // S2MM receive from host injection
        let external_frame = vec![0xAB; 32];
        handle.push_s2mm(external_frame);

        vdma.write32(S2MM_START_ADDRESS_BASE, 0x2000, 0xF);
        vdma.write32(S2MM_FRMDLY_STRIDE, 32, 0xF);
        vdma.write32(S2MM_HSIZE, 32, 0xF);
        vdma.write32(S2MM_VDMACR, CR_RS, 0xF);
        vdma.write32(S2MM_VSIZE, 1, 0xF);
        vdma.service(200, &mut mem);

        assert_eq!(&mem.data[0x2000..0x2020], &[0xAB; 32]);
    }
}
