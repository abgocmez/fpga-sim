//! AXI DMA (`xilinx.com:ip:axi_dma:7.1`, Xilinx PG021), register level: direct register
//! (simple) mode and scatter-gather mode with a linear descriptor chain.
//!
//! Register map (offsets from the base address; recalled from PG021, **not** checked against
//! hardware or a Xilinx simulation):
//!
//! | offset | MM2S | offset | S2MM |
//! |---|---|---|---|
//! | `0x00` | `MM2S_DMACR` | `0x30` | `S2MM_DMACR` |
//! | `0x04` | `MM2S_DMASR` | `0x34` | `S2MM_DMASR` |
//! | `0x08`/`0x0C` | `MM2S_CURDESC`(`_MSB`), SG only | `0x38`/`0x3C` | `S2MM_CURDESC`(`_MSB`) |
//! | `0x10`/`0x14` | `MM2S_TAILDESC`(`_MSB`), SG only | `0x40`/`0x44` | `S2MM_TAILDESC`(`_MSB`) |
//! | `0x18`/`0x1C` | `MM2S_SA`(`_MSB`), direct mode | `0x48`/`0x4C` | `S2MM_DA`(`_MSB`) |
//! | `0x28` | `MM2S_LENGTH`, direct mode | `0x58` | `S2MM_LENGTH` |
//! | `0x2C` | `SG_CTL`, SG only | | |
//!
//! `DMACR`: bit 0 `RS` (run/stop), bit 2 `Reset` (self clearing, resets the whole core), bit 3
//! keyhole (stored, not modelled), bit 4 cyclic BD (stored, not modelled), bit 12 `IOC_IrqEn`,
//! bit 13 `Dly_IrqEn`, bit 14 `Err_IrqEn`, bits 23:16 `IRQThreshold`, bits 31:24 `IRQDelay`.
//! `DMASR`: bit 0 `Halted`, bit 1 `Idle`, bit 3 `SGIncld`, bit 4 `DMAIntErr`, bit 5
//! `DMASlvErr`, bit 6 `DMADecErr`, bits 8..10 `SGIntErr`/`SGSlvErr`/`SGDecErr`, bit 12
//! `IOC_Irq`, bit 13 `Dly_Irq`, bit 14 `Err_Irq` (the three interrupt bits are write-1-to-clear),
//! bits 23:16 `IRQThresholdSts`, bits 31:24 `IRQDelaySts`.
//!
//! Descriptor (SG, 64-byte aligned): `NXTDESC` `+0x00`, `BUFFER_ADDRESS` `+0x08`, `CONTROL`
//! `+0x18` (bits 25:0 buffer length, bit 27 `TXSOF`, bit 26 `TXEOF`), `STATUS` `+0x1C` (bits
//! 25:0 transferred, bit 27 `RXSOF`, bit 26 `RXEOF`, bits 30..28 `DMADecErr`/`DMASlvErr`/
//! `DMAIntErr`, bit 31 `Cmplt`). The MSB words are accepted and ignored (32-bit addresses).
//!
//! Modelled: the register file; a direct transfer started by a `LENGTH` write (the model reads
//! or writes PS memory through the HP port of the project, `PS_PORT`, in one go and signals the
//! completion after the number of AXI beats it takes at the AXI clock); descriptor chains with
//! the tail-pointer protocol; packets (`TLAST`) on the stream side; `IOC`, `Dly` and `Err`
//! interrupts with the interrupt threshold and delay timer; error bits for failed memory
//! accesses; soft reset; the halted and idle bits.
//!
//! The stream side is a queue of packets. With `STREAM_LOOPBACK` (the default, what the XSA
//! importer sets when the block design connects `M_AXIS_MM2S` to `S_AXIS_S2MM`, directly or
//! through `axis_*` plumbing) every MM2S packet appears at S2MM; without it the packets leave
//! through [`DmaHandle::take_mm2s`] and enter through [`DmaHandle::push_s2mm`] (the host or a
//! test is the stream peer). There is no stream connection to user RTL in XSim.
//!
//! Not modelled: multi-channel mode and micro DMA (the instance is rejected), the status/
//! control streams (`C_SG_INCLUDE_STSCNTRL_STRM`), cyclic BDs, keyhole, unaligned transfers
//! (the DRE is irrelevant here: byte addressing is exact), 64-bit addresses, AXI protocol
//! detail (bursts, outstanding transactions, 4 KiB crossing).
//!
//! `VERIFY:` items:
//! * Reset values: `DMACR = 0x00010002` (this one and every offset, name and field position
//!   above is confirmed by the register metadata in a Vivado 2022.1 hardware handoff),
//!   `DMASR = 0x00010001` (`0x00010009` with SG): the metadata says `0x00010000`, which omits
//!   `Halted`; real cores read `Halted = 1` after a reset.
//! * `Idle` reads 1 as soon as `RS` is set and nothing is in flight (also in SG mode before any
//!   tail pointer write); `Halted` and `Idle` are never both 1.
//! * A `LENGTH` write of 0, or one while the channel is not running, halted or busy, is ignored
//!   except that zero length sets `DMAIntErr` and halts; the real core's reaction differs by
//!   mode.
//! * `LENGTH` is `C_SG_LENGTH_WIDTH` bits wide; higher bits are dropped.
//! * S2MM `LENGTH` reads the number of bytes received after completion (it is zero while idle
//!   before the first transfer), and holds that until the next `LENGTH` write.
//! * An S2MM transfer ends at `TLAST` or when the buffer is full. A longer packet continues in
//!   the next transfer (it is not dropped).
//! * `IRQThresholdSts` reads the number of completions still needed; `IRQDelaySts` reads 0. An
//!   `IRQThreshold` of 0 behaves as 1. The delay timer counts `IRQDelay * 125` AXI clocks.
//! * The `Reset` bit clears within the register write (the real core takes some clocks).
//! * A fetched descriptor whose `Cmplt` bit is already set raises `SGIntErr`.
//! * Registers of a missing channel, unknown offsets, and SG registers on a non-SG core read 0.

use crate::params::{Params, flag, merge, number, ranged};
use pl_api::{BusError, MasterAccess, PsMasterPort, PsPort};
use pl_native::RegisterBlock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `MM2S_DMACR`.
pub const MM2S_DMACR: u64 = 0x00;
/// `MM2S_DMASR`.
pub const MM2S_DMASR: u64 = 0x04;
/// `MM2S_CURDESC`.
pub const MM2S_CURDESC: u64 = 0x08;
/// `MM2S_TAILDESC`.
pub const MM2S_TAILDESC: u64 = 0x10;
/// `MM2S_SA`.
pub const MM2S_SA: u64 = 0x18;
/// `MM2S_LENGTH`.
pub const MM2S_LENGTH: u64 = 0x28;
/// `SG_CTL`.
pub const SG_CTL: u64 = 0x2C;
/// `S2MM_DMACR`.
pub const S2MM_DMACR: u64 = 0x30;
/// `S2MM_DMASR`.
pub const S2MM_DMASR: u64 = 0x34;
/// `S2MM_CURDESC`.
pub const S2MM_CURDESC: u64 = 0x38;
/// `S2MM_TAILDESC`.
pub const S2MM_TAILDESC: u64 = 0x40;
/// `S2MM_DA`.
pub const S2MM_DA: u64 = 0x48;
/// `S2MM_LENGTH`.
pub const S2MM_LENGTH: u64 = 0x58;

/// `DMACR` bit: run/stop.
pub const CR_RS: u32 = 1 << 0;
/// `DMACR` bit: soft reset.
pub const CR_RESET: u32 = 1 << 2;
/// `DMACR` bit: keyhole.
pub const CR_KEYHOLE: u32 = 1 << 3;
/// `DMACR` bit: cyclic BD.
pub const CR_CYCLIC: u32 = 1 << 4;
/// `DMACR` bit: interrupt on complete enable.
pub const CR_IOC_EN: u32 = 1 << 12;
/// `DMACR` bit: delay interrupt enable.
pub const CR_DLY_EN: u32 = 1 << 13;
/// `DMACR` bit: error interrupt enable.
pub const CR_ERR_EN: u32 = 1 << 14;

/// `DMASR` bit: halted.
pub const SR_HALTED: u32 = 1 << 0;
/// `DMASR` bit: idle.
pub const SR_IDLE: u32 = 1 << 1;
/// `DMASR` bit: scatter-gather included.
pub const SR_SG_INCLD: u32 = 1 << 3;
/// `DMASR` bit: DMA internal error.
pub const SR_DMA_INT_ERR: u32 = 1 << 4;
/// `DMASR` bit: DMA slave error.
pub const SR_DMA_SLV_ERR: u32 = 1 << 5;
/// `DMASR` bit: DMA decode error.
pub const SR_DMA_DEC_ERR: u32 = 1 << 6;
/// `DMASR` bit: SG internal error.
pub const SR_SG_INT_ERR: u32 = 1 << 8;
/// `DMASR` bit: SG slave error.
pub const SR_SG_SLV_ERR: u32 = 1 << 9;
/// `DMASR` bit: SG decode error.
pub const SR_SG_DEC_ERR: u32 = 1 << 10;
/// `DMASR` bit: interrupt on complete.
pub const SR_IOC_IRQ: u32 = 1 << 12;
/// `DMASR` bit: delay interrupt.
pub const SR_DLY_IRQ: u32 = 1 << 13;
/// `DMASR` bit: error interrupt.
pub const SR_ERR_IRQ: u32 = 1 << 14;

/// Descriptor control bit: start of packet (MM2S).
pub const BD_CTRL_SOF: u32 = 1 << 27;
/// Descriptor control bit: end of packet (MM2S).
pub const BD_CTRL_EOF: u32 = 1 << 26;
/// Descriptor status bit: complete.
pub const BD_STS_CMPLT: u32 = 1 << 31;
/// Descriptor status bit: decode error.
pub const BD_STS_DEC_ERR: u32 = 1 << 30;
/// Descriptor status bit: slave error.
pub const BD_STS_SLV_ERR: u32 = 1 << 29;
/// Descriptor status bit: internal error.
pub const BD_STS_INT_ERR: u32 = 1 << 28;
/// Descriptor status bit: start of packet received (S2MM).
pub const BD_STS_SOF: u32 = 1 << 27;
/// Descriptor status bit: end of packet received (S2MM).
pub const BD_STS_EOF: u32 = 1 << 26;

const CR_RESET_VALUE: u32 = 0x0001_0002;
/// Size of one memory transaction toward the PS, bytes.
const CHUNK: usize = 4096;
/// AXI clocks of fixed cost per transfer or descriptor (request, response, write-back).
const OVERHEAD_CYCLES: u64 = 16;

/// The IP's configuration (`C_*` parameters plus the project's `PS_PORT` and
/// `STREAM_LOOPBACK`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaConfig {
    /// `C_INCLUDE_SG`.
    pub include_sg: bool,
    /// `C_INCLUDE_MM2S`.
    pub mm2s: bool,
    /// `C_INCLUDE_S2MM`.
    pub s2mm: bool,
    /// `C_SG_LENGTH_WIDTH`, 8 to 26.
    pub length_width: u32,
    /// `C_M_AXI_MM2S_DATA_WIDTH` and `C_M_AXI_S2MM_DATA_WIDTH` in bits.
    pub data_width: [u32; 2],
    /// AXI clock frequency in Hz (`CLK_FREQ_HZ`).
    pub clock_hz: u64,
    /// The PS port the memory masters reach DDR through (`PS_PORT`, default `S_AXI_HP0`).
    pub ps_port: PsMasterPort,
    /// MM2S stream feeds S2MM (`STREAM_LOOPBACK`, default 1).
    pub loopback: bool,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for DmaConfig {
    fn default() -> Self {
        DmaConfig {
            include_sg: false,
            mm2s: true,
            s2mm: true,
            length_width: 14,
            data_width: [32, 32],
            clock_hz: 100_000_000,
            ps_port: PsMasterPort::SAxiHp0,
            loopback: true,
            window: 0x1_0000,
        }
    }
}

/// Parses `S_AXI_HP0`, `HP1`, `S_AXI_GP0`, `ACP` (any case).
pub fn parse_ps_port(text: &str) -> Result<PsMasterPort, String> {
    let t = text.trim().to_ascii_uppercase();
    let t = t.strip_prefix("S_AXI_").unwrap_or(&t);
    Ok(match t {
        "HP0" => PsMasterPort::SAxiHp0,
        "HP1" => PsMasterPort::SAxiHp1,
        "HP2" => PsMasterPort::SAxiHp2,
        "HP3" => PsMasterPort::SAxiHp3,
        "GP0" => PsMasterPort::SAxiGp0,
        "GP1" => PsMasterPort::SAxiGp1,
        "ACP" => PsMasterPort::SAxiAcp,
        _ => {
            return Err(format!(
                "PS_PORT `{text}` is not a PS slave port (S_AXI_HP0..3, S_AXI_GP0/1, S_AXI_ACP)"
            ));
        }
    })
}

impl DmaConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<DmaConfig, String> {
        if flag(p, "C_ENABLE_MULTI_CHANNEL", false)? {
            return Err("multi-channel mode (C_ENABLE_MULTI_CHANNEL) is not modelled".to_string());
        }
        if flag(p, "C_MICRO_DMA", false)? {
            return Err("micro DMA (C_MICRO_DMA) is not modelled".to_string());
        }
        let width = |key: &str| -> Result<u32, String> {
            let w = ranged(p, key, 32, 8, 1024)? as u32;
            if w.is_power_of_two() {
                Ok(w)
            } else {
                Err(format!("parameter {key} = {w} is not a power of two"))
            }
        };
        Ok(DmaConfig {
            include_sg: flag(p, "C_INCLUDE_SG", false)?,
            mm2s: flag(p, "C_INCLUDE_MM2S", true)?,
            s2mm: flag(p, "C_INCLUDE_S2MM", true)?,
            length_width: ranged(p, "C_SG_LENGTH_WIDTH", 14, 8, 26)? as u32,
            data_width: [
                width("C_M_AXI_MM2S_DATA_WIDTH")?,
                width("C_M_AXI_S2MM_DATA_WIDTH")?,
            ],
            clock_hz: ranged(p, "CLK_FREQ_HZ", 100_000_000, 1, 1_000_000_000)?,
            ps_port: match p.get("PS_PORT") {
                Some(v) => parse_ps_port(v)?,
                None => PsMasterPort::SAxiHp0,
            },
            loopback: number(p, "STREAM_LOOPBACK", 1)? != 0,
            window,
        })
    }

    fn length_mask(&self) -> u32 {
        ((1u64 << self.length_width) - 1) as u32
    }

    fn clk_ns(&self) -> f64 {
        1e9 / self.clock_hz as f64
    }
}

/// A transfer started by a `LENGTH` write, waiting to be carried out.
#[derive(Clone, Copy, Debug)]
struct Cmd {
    addr: u64,
    len: u32,
}

/// What happens when a transfer's time has come.
#[derive(Debug, Default)]
struct Event {
    when: u64,
    /// Counts toward the interrupt threshold.
    ioc: bool,
    /// New value of the S2MM `LENGTH` register.
    length: Option<u32>,
    /// MM2S packet that leaves on the stream.
    release: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
struct Chan {
    cr: u32,
    rs: bool,
    err_bits: u32,
    ioc: bool,
    dly: bool,
    err_irq: bool,
    halted_err: bool,
    addr: u64,
    length: u32,
    curdesc: u64,
    taildesc: u64,
    sg_ctl: u32,
    cmd: Option<Cmd>,
    events: VecDeque<Event>,
    /// Time the datapath is free again.
    last_done: u64,
    thr_done: u32,
    dly_deadline: Option<u64>,
    // Scatter-gather engine.
    sg_active: bool,
    /// Address of the next descriptor to fetch.
    sg_next: u64,
    /// Address of the last descriptor completed.
    sg_last: Option<u64>,
    /// A new tail pointer restarted the engine: re-read `NXTDESC` of the last descriptor.
    sg_refresh: bool,
    /// MM2S packet being gathered over several descriptors.
    gather: Vec<u8>,
    /// S2MM: the current packet already delivered its first byte to a descriptor.
    rx_started: bool,
}

impl Chan {
    fn threshold(&self) -> u32 {
        ((self.cr >> 16) & 0xFF).max(1)
    }

    fn delay(&self) -> u32 {
        (self.cr >> 24) & 0xFF
    }

    fn busy(&self) -> bool {
        self.cmd.is_some() || !self.events.is_empty() || self.sg_active
    }

    fn halted(&self) -> bool {
        self.halted_err || (!self.rs && !self.busy())
    }

    fn dmasr(&self, sg: bool) -> u32 {
        let mut v = 0;
        if self.halted() {
            v |= SR_HALTED;
        } else if self.rs && !self.busy() {
            v |= SR_IDLE;
        }
        if sg {
            v |= SR_SG_INCLD;
        }
        v |= self.err_bits;
        if self.ioc {
            v |= SR_IOC_IRQ;
        }
        if self.dly {
            v |= SR_DLY_IRQ;
        }
        if self.err_irq {
            v |= SR_ERR_IRQ;
        }
        v |= self.threshold().saturating_sub(self.thr_done) << 16;
        v
    }

    fn irq(&self) -> bool {
        (self.ioc && self.cr & CR_IOC_EN != 0)
            || (self.dly && self.cr & CR_DLY_EN != 0)
            || (self.err_irq && self.cr & CR_ERR_EN != 0)
    }

    fn fail(&mut self, bit: u32) {
        self.err_bits |= bit;
        self.err_irq = true;
        self.halted_err = true;
        self.cmd = None;
        self.sg_active = false;
    }

    fn reset(&mut self) {
        *self = Chan {
            cr: CR_RESET_VALUE,
            ..Chan::default()
        };
    }
}

/// Counters for tests and reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DmaStats {
    /// MM2S packets sent on the stream.
    pub mm2s_packets: u64,
    /// MM2S payload bytes read from memory.
    pub mm2s_bytes: u64,
    /// S2MM payload bytes written to memory.
    pub s2mm_bytes: u64,
    /// Descriptors completed (both channels).
    pub descriptors: u64,
}

struct State {
    cfg: DmaConfig,
    ch: [Chan; 2],
    now: u64,
    to_s2mm: VecDeque<Vec<u8>>,
    from_mm2s: VecDeque<Vec<u8>>,
    log: Vec<MasterAccess>,
    stats: DmaStats,
}

impl State {
    fn power_on(cfg: DmaConfig) -> State {
        let mut s = State {
            cfg,
            ch: [Chan::default(), Chan::default()],
            now: 0,
            to_s2mm: VecDeque::new(),
            from_mm2s: VecDeque::new(),
            log: Vec::new(),
            stats: DmaStats::default(),
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.ch[0].reset();
        self.ch[1].reset();
        self.to_s2mm.clear();
        self.from_mm2s.clear();
    }

    fn present(&self, ch: usize) -> bool {
        if ch == 0 {
            self.cfg.mm2s
        } else {
            self.cfg.s2mm
        }
    }

    fn cycles_ns(&self, cycles: u64) -> u64 {
        (cycles as f64 * self.cfg.clk_ns()).ceil() as u64
    }

    fn beats(&self, ch: usize, bytes: u64) -> u64 {
        let w = u64::from(self.cfg.data_width[ch] / 8);
        bytes.div_ceil(w)
    }

    fn read_mem(
        &mut self,
        addr: u64,
        len: usize,
        ps: &mut dyn PsPort,
    ) -> Result<Vec<u8>, BusError> {
        let mut out = vec![0u8; len];
        let mut result = Ok(());
        for (i, chunk) in out.chunks_mut(CHUNK).enumerate() {
            let a = addr + (i * CHUNK) as u64;
            let r = ps.mem_read(self.cfg.ps_port, a, chunk);
            self.record(a, chunk.len(), false, chunk, r.is_ok());
            if r.is_err() {
                result = r;
                break;
            }
        }
        result.map(|()| out)
    }

    fn write_mem(&mut self, addr: u64, data: &[u8], ps: &mut dyn PsPort) -> Result<(), BusError> {
        for (i, chunk) in data.chunks(CHUNK).enumerate() {
            let a = addr + (i * CHUNK) as u64;
            let r = ps.mem_write(self.cfg.ps_port, a, chunk);
            self.record(a, chunk.len(), true, chunk, r.is_ok());
            r?;
        }
        Ok(())
    }

    fn record(&mut self, addr: u64, len: usize, write: bool, data: &[u8], ok: bool) {
        let mut first = [0u8; 8];
        let n = data.len().min(8);
        first[..n].copy_from_slice(&data[..n]);
        self.log.push(MasterAccess {
            t_ns: self.now,
            port: self.cfg.ps_port,
            write,
            addr,
            len: len as u32,
            data: u64::from_le_bytes(first),
            ok,
        });
    }

    fn err_for(e: BusError, dma: bool) -> u32 {
        match (e, dma) {
            (BusError::Decode, true) => SR_DMA_DEC_ERR,
            (BusError::Slave, true) => SR_DMA_SLV_ERR,
            (BusError::Decode, false) => SR_SG_DEC_ERR,
            (BusError::Slave, false) => SR_SG_SLV_ERR,
        }
    }

    /// Applies the events whose time has come and the delay timers.
    fn complete_due(&mut self) {
        for ci in 0..2 {
            while self.ch[ci]
                .events
                .front()
                .is_some_and(|e| e.when <= self.now)
            {
                let Some(e) = self.ch[ci].events.pop_front() else {
                    break;
                };
                if let Some(l) = e.length {
                    self.ch[ci].length = l;
                }
                if let Some(pkt) = e.release {
                    self.stats.mm2s_packets += 1;
                    if self.cfg.loopback {
                        self.to_s2mm.push_back(pkt);
                    } else {
                        self.from_mm2s.push_back(pkt);
                    }
                }
                if e.ioc {
                    self.count_completion(ci, e.when);
                }
            }
            let c = &mut self.ch[ci];
            if let Some(d) = c.dly_deadline {
                if d <= self.now {
                    c.dly_deadline = None;
                    c.thr_done = 0;
                    c.dly = true;
                }
            }
        }
    }

    fn count_completion(&mut self, ci: usize, when: u64) {
        let delay_ns = {
            let d = u64::from(self.ch[ci].delay());
            self.cycles_ns(d * 125)
        };
        let c = &mut self.ch[ci];
        c.thr_done += 1;
        if c.thr_done >= c.threshold() {
            c.thr_done = 0;
            c.dly_deadline = None;
            c.ioc = true;
        } else if c.delay() != 0 && c.dly_deadline.is_none() {
            c.dly_deadline = Some(when + delay_ns);
        }
    }

    fn start_direct(&mut self, ci: usize, len: u32) {
        let mask = self.cfg.length_mask();
        let len = len & mask;
        let (sg, c) = (self.cfg.include_sg, &mut self.ch[ci]);
        if sg || !c.rs || c.halted_err || c.busy() {
            return;
        }
        if len == 0 {
            c.fail(SR_DMA_INT_ERR);
            return;
        }
        c.cmd = Some(Cmd { addr: c.addr, len });
        if ci == 1 {
            c.length = 0;
        }
    }

    /// Carries out the memory side of the work that is ready.
    fn run(&mut self, ps: &mut dyn PsPort) {
        for ci in 0..2 {
            if !self.present(ci) {
                continue;
            }
            self.run_direct(ci, ps);
            if self.cfg.include_sg {
                self.run_sg(ci, ps);
            }
        }
    }

    fn run_direct(&mut self, ci: usize, ps: &mut dyn PsPort) {
        let Some(cmd) = self.ch[ci].cmd else { return };
        let len = cmd.len as usize;
        let start = self.now.max(self.ch[ci].last_done);
        let dur = self.cycles_ns(OVERHEAD_CYCLES + self.beats(ci, u64::from(cmd.len)));
        if ci == 0 {
            match self.read_mem(cmd.addr, len, ps) {
                Ok(data) => {
                    self.ch[0].cmd = None;
                    self.stats.mm2s_bytes += len as u64;
                    self.ch[0].last_done = start + dur;
                    self.ch[0].events.push_back(Event {
                        when: start + dur,
                        ioc: true,
                        release: Some(data),
                        ..Event::default()
                    });
                }
                Err(e) => self.ch[0].fail(Self::err_for(e, true)),
            }
        } else {
            let Some(pkt) = self.to_s2mm.front_mut() else {
                return; // wait for data on the stream
            };
            let n = len.min(pkt.len());
            let data: Vec<u8> = pkt.drain(..n).collect();
            if pkt.is_empty() {
                self.to_s2mm.pop_front();
            }
            match self.write_mem(cmd.addr, &data, ps) {
                Ok(()) => {
                    self.ch[1].cmd = None;
                    self.stats.s2mm_bytes += n as u64;
                    let dur = self.cycles_ns(OVERHEAD_CYCLES + self.beats(1, n as u64));
                    self.ch[1].last_done = start + dur;
                    self.ch[1].events.push_back(Event {
                        when: start + dur,
                        ioc: true,
                        length: Some(n as u32),
                        ..Event::default()
                    });
                }
                Err(e) => self.ch[1].fail(Self::err_for(e, true)),
            }
        }
    }

    fn run_sg(&mut self, ci: usize, ps: &mut dyn PsPort) {
        // Bounded: one call never walks more than a thousand descriptors.
        for _ in 0..1000 {
            let c = &self.ch[ci];
            if !c.sg_active || c.halted_err || !c.rs {
                return;
            }
            if c.sg_refresh {
                // Software may have linked a new descriptor after the old tail since the
                // engine read it: fetch the link again.
                let last = c.sg_last.unwrap_or(0);
                let mut w = [0u8; 4];
                let r = ps.mem_read(self.cfg.ps_port, last, &mut w);
                self.record(last, 4, false, &w, r.is_ok());
                if let Err(e) = r {
                    self.ch[ci].fail(Self::err_for(e, false));
                    return;
                }
                self.ch[ci].sg_next = u64::from(u32::from_le_bytes(w));
                self.ch[ci].sg_refresh = false;
            }
            let bd = self.ch[ci].sg_next;
            let mut raw = [0u8; 0x20];
            let r = ps.mem_read(self.cfg.ps_port, bd, &mut raw);
            self.record(bd, raw.len(), false, &raw, r.is_ok());
            if let Err(e) = r {
                self.ch[ci].fail(Self::err_for(e, false));
                return;
            }
            let word = |o: usize| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
            let (next, buf, ctrl, status) = (word(0x00), word(0x08), word(0x18), word(0x1C));
            if status & BD_STS_CMPLT != 0 {
                self.ch[ci].fail(SR_SG_INT_ERR);
                return;
            }
            let len = (ctrl & 0x03FF_FFFF) as usize;
            if len == 0 {
                self.write_status(ci, bd, BD_STS_INT_ERR, ps);
                self.ch[ci].fail(SR_DMA_INT_ERR);
                return;
            }
            let start = self.now.max(self.ch[ci].last_done);
            let mut done_status = BD_STS_CMPLT;
            let (event_len, release);
            let n;
            if ci == 0 {
                let data = match self.read_mem(u64::from(buf), len, ps) {
                    Ok(d) => d,
                    Err(e) => {
                        self.write_status(ci, bd, bd_err(e), ps);
                        self.ch[0].fail(Self::err_for(e, true));
                        return;
                    }
                };
                n = len;
                self.stats.mm2s_bytes += len as u64;
                let c = &mut self.ch[0];
                if ctrl & BD_CTRL_SOF != 0 {
                    c.gather.clear();
                }
                c.gather.extend_from_slice(&data);
                release = (ctrl & BD_CTRL_EOF != 0).then(|| std::mem::take(&mut c.gather));
                event_len = None;
            } else {
                let Some(pkt) = self.to_s2mm.front_mut() else {
                    return; // wait for stream data; the descriptor stays unfetched
                };
                n = len.min(pkt.len());
                let data: Vec<u8> = pkt.drain(..n).collect();
                let eof = pkt.is_empty();
                if eof {
                    self.to_s2mm.pop_front();
                }
                if let Err(e) = self.write_mem(u64::from(buf), &data, ps) {
                    self.write_status(ci, bd, bd_err(e), ps);
                    self.ch[1].fail(Self::err_for(e, true));
                    return;
                }
                self.stats.s2mm_bytes += n as u64;
                if !self.ch[1].rx_started {
                    done_status |= BD_STS_SOF;
                }
                self.ch[1].rx_started = !eof;
                if eof {
                    done_status |= BD_STS_EOF;
                }
                event_len = Some(n as u32);
                release = None;
            }
            self.write_status(ci, bd, done_status | n as u32, ps);
            self.stats.descriptors += 1;
            let dur = self.cycles_ns(2 * OVERHEAD_CYCLES + self.beats(ci, n as u64));
            let c = &mut self.ch[ci];
            c.last_done = start + dur;
            c.events.push_back(Event {
                when: start + dur,
                ioc: true,
                length: event_len,
                release,
            });
            c.curdesc = bd;
            c.sg_last = Some(bd);
            c.sg_next = u64::from(next);
            if bd == c.taildesc {
                c.sg_active = false;
            }
        }
    }

    fn write_status(&mut self, _ci: usize, bd: u64, status: u32, ps: &mut dyn PsPort) {
        let b = status.to_le_bytes();
        let r = ps.mem_write(self.cfg.ps_port, bd + 0x1C, &b);
        self.record(bd + 0x1C, 4, true, &b, r.is_ok());
    }

    fn sg_kick(&mut self, ci: usize) {
        let c = &mut self.ch[ci];
        if !self.cfg.include_sg || !c.rs || c.halted_err || c.sg_active {
            return;
        }
        if c.taildesc != 0 && c.sg_last != Some(c.taildesc) && c.sg_next != 0 {
            c.sg_active = true;
            c.sg_refresh = c.sg_last.is_some();
        }
    }

    fn read_reg(&self, offset: u64) -> u32 {
        let (ci, off) = match offset {
            0x00..=0x2F => (0, offset),
            0x30..=0x5F => (1, offset - 0x30),
            _ => return 0,
        };
        if !self.present(ci) {
            return 0;
        }
        let c = &self.ch[ci];
        let sg = self.cfg.include_sg;
        match off {
            0x00 => c.cr & !CR_RESET,
            0x04 => c.dmasr(sg),
            0x08 if sg => c.sg_next_or_cur(),
            0x0C if sg => 0,
            0x10 if sg => c.taildesc as u32,
            0x14 if sg => 0,
            0x18 if !sg => c.addr as u32,
            0x1C if !sg => 0,
            0x28 if !sg => c.length,
            0x2C if sg && ci == 0 => c.sg_ctl,
            _ => 0,
        }
    }

    fn write_reg(&mut self, offset: u64, value: u32, strobe: u8) {
        let (ci, off) = match offset {
            0x00..=0x2F => (0, offset),
            0x30..=0x5F => (1, offset - 0x30),
            _ => return,
        };
        if !self.present(ci) {
            return;
        }
        let sg = self.cfg.include_sg;
        match off {
            0x00 => {
                let new = merge(self.ch[ci].cr, value, strobe);
                if new & CR_RESET != 0 {
                    self.reset();
                    return;
                }
                let c = &mut self.ch[ci];
                let keep = CR_RS | CR_KEYHOLE | CR_CYCLIC | CR_IOC_EN | CR_DLY_EN | CR_ERR_EN;
                // Bit 1 is reserved and reads 1 (reset value 0x10002 in Vivado's register metadata).
                c.cr = (new & (keep | 0xFFFF_0000)) | 0x2;
                let run = new & CR_RS != 0;
                if run && !c.rs {
                    c.rs = true;
                    // Run after a halt for an error is only possible through a reset.
                    self.sg_kick(ci);
                } else if !run && c.rs {
                    c.rs = false;
                }
            }
            0x04 => {
                let c = &mut self.ch[ci];
                if value & SR_IOC_IRQ != 0 {
                    c.ioc = false;
                }
                if value & SR_DLY_IRQ != 0 {
                    c.dly = false;
                }
                if value & SR_ERR_IRQ != 0 {
                    c.err_irq = false;
                }
            }
            0x08 if sg => {
                let c = &mut self.ch[ci];
                // Only writable while the engine is not running.
                if !c.busy() {
                    let v = u64::from(merge(c.sg_next as u32, value, strobe)) & !0x3F;
                    c.sg_next = v;
                    c.curdesc = v;
                    c.sg_last = None;
                }
            }
            0x10 if sg => {
                let c = &mut self.ch[ci];
                c.taildesc = u64::from(merge(c.taildesc as u32, value, strobe)) & !0x3F;
                self.sg_kick(ci);
            }
            0x18 if !sg => {
                let c = &mut self.ch[ci];
                c.addr = u64::from(merge(c.addr as u32, value, strobe));
            }
            0x28 if !sg => self.start_direct(ci, value),
            0x2C if sg && ci == 0 => self.ch[0].sg_ctl = merge(self.ch[0].sg_ctl, value, strobe),
            _ => {}
        }
    }
}

fn bd_err(e: BusError) -> u32 {
    match e {
        BusError::Decode => BD_STS_DEC_ERR,
        BusError::Slave => BD_STS_SLV_ERR,
    }
}

impl Chan {
    /// `CURDESC` reads the descriptor being (or last) worked on.
    fn sg_next_or_cur(&self) -> u32 {
        if self.sg_active || self.sg_last.is_none() {
            self.sg_next as u32
        } else {
            self.curdesc as u32
        }
    }
}

/// An AXI DMA instance as a [`RegisterBlock`].
pub struct AxiDma {
    name: String,
    st: Arc<Mutex<State>>,
}

/// A clonable handle to the stream side, the master log and the counters of an [`AxiDma`].
#[derive(Clone)]
pub struct DmaHandle {
    st: Arc<Mutex<State>>,
}

impl AxiDma {
    /// A freshly reset instance named `name` and the handle to its stream side.
    pub fn new(name: &str, cfg: DmaConfig) -> (AxiDma, DmaHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg)));
        (
            AxiDma {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            DmaHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl DmaHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance's configuration.
    pub fn config(&self) -> DmaConfig {
        self.lock().cfg.clone()
    }

    /// Removes and returns the packets MM2S sent on its stream (only without
    /// `STREAM_LOOPBACK`).
    pub fn take_mm2s(&self) -> Vec<Vec<u8>> {
        self.lock().from_mm2s.drain(..).collect()
    }

    /// Sends a packet (ending in `TLAST`) into the S2MM stream.
    pub fn push_s2mm(&self, packet: Vec<u8>) {
        if !packet.is_empty() {
            self.lock().to_s2mm.push_back(packet);
        }
    }

    /// Removes and returns the memory accesses the DMA made since the last call.
    pub fn take_log(&self) -> Vec<MasterAccess> {
        std::mem::take(&mut self.lock().log)
    }

    /// The counters.
    pub fn stats(&self) -> DmaStats {
        self.lock().stats
    }

    /// `(mm2s, s2mm)` `DMASR` as the registers read now.
    pub fn status(&self) -> (u32, u32) {
        let s = self.lock();
        let sg = s.cfg.include_sg;
        (s.ch[0].dmasr(sg), s.ch[1].dmasr(sg))
    }
}

impl RegisterBlock for AxiDma {
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
        s.complete_due();
    }

    fn irq_level(&self) -> bool {
        self.irq_output(0)
    }

    fn irq_output(&self, output: usize) -> bool {
        let s = self.lock();
        s.present(output.min(1)) && output < 2 && s.ch[output].irq()
    }

    fn service(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        let mut s = self.lock();
        s.now = s.now.max(now_ns);
        s.complete_due();
        s.run(ps);
        s.complete_due();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A flat memory of 64 KiB behind the PS port.
    struct Mem {
        bytes: Vec<u8>,
        fail_above: Option<u64>,
        slave_error_at: Option<u64>,
    }
    impl Mem {
        fn new() -> Mem {
            Mem {
                bytes: vec![0; 0x1_0000],
                fail_above: None,
                slave_error_at: None,
            }
        }
        fn check(&self, addr: u64, len: usize) -> Result<(), BusError> {
            if self.fail_above.is_some_and(|f| addr + len as u64 > f) {
                return Err(BusError::Decode);
            }
            if self.slave_error_at == Some(addr) {
                return Err(BusError::Slave);
            }
            Ok(())
        }
        fn word(&self, addr: u64) -> u32 {
            let a = addr as usize;
            u32::from_le_bytes([
                self.bytes[a],
                self.bytes[a + 1],
                self.bytes[a + 2],
                self.bytes[a + 3],
            ])
        }
        fn put_word(&mut self, addr: u64, v: u32) {
            let a = addr as usize;
            self.bytes[a..a + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    impl PsPort for Mem {
        fn mem_read(
            &mut self,
            _: PsMasterPort,
            addr: u64,
            buf: &mut [u8],
        ) -> pl_api::BusResult<()> {
            self.check(addr, buf.len())?;
            buf.copy_from_slice(&self.bytes[addr as usize..addr as usize + buf.len()]);
            Ok(())
        }
        fn mem_write(&mut self, _: PsMasterPort, addr: u64, data: &[u8]) -> pl_api::BusResult<()> {
            self.check(addr, data.len())?;
            self.bytes[addr as usize..addr as usize + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn irq(&mut self, _: u32, _: bool, _: u64) -> pl_api::BusResult<()> {
            Ok(())
        }
    }

    fn params(p: &[(&str, &str)]) -> Params {
        p.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<BTreeMap<_, _>>()
    }

    fn dma(p: &[(&str, &str)]) -> (AxiDma, DmaHandle) {
        AxiDma::new("dma", DmaConfig::from_params(&params(p), 0x1_0000).unwrap())
    }

    fn wr(d: &mut AxiDma, off: u64, v: u32) {
        d.write32(off, v, 0xF);
    }

    /// Runs `ns` of simulated time in 1 us steps, servicing the block like NativePl does.
    fn run(d: &mut AxiDma, m: &mut Mem, from: u64, ns: u64) -> u64 {
        let mut t = from;
        while t < from + ns {
            t += 1_000;
            d.advance(t);
            d.service(t, m);
        }
        t
    }

    #[test]
    fn reset_values_and_config() {
        let (mut d, _) = dma(&[]);
        assert_eq!(d.read32(MM2S_DMACR), 0x0001_0002);
        assert_eq!(d.read32(MM2S_DMASR), 0x0001_0001);
        assert_eq!(d.read32(S2MM_DMASR), 0x0001_0001);
        let (mut g, _) = dma(&[("C_INCLUDE_SG", "1")]);
        assert_eq!(g.read32(MM2S_DMASR), 0x0001_0009);
        let e = DmaConfig::from_params(&params(&[("C_ENABLE_MULTI_CHANNEL", "1")]), 0x1000);
        assert!(e.unwrap_err().contains("multi-channel"));
        let e = DmaConfig::from_params(&params(&[("PS_PORT", "S_AXI_HP9")]), 0x1000);
        assert!(e.unwrap_err().contains("PS_PORT"));
        let c = DmaConfig::from_params(&params(&[("PS_PORT", "S_AXI_HP1")]), 0x1000).unwrap();
        assert_eq!(c.ps_port, PsMasterPort::SAxiHp1);
    }

    #[test]
    fn run_stop_halted_and_idle() {
        let (mut d, _) = dma(&[]);
        wr(&mut d, MM2S_DMACR, CR_RS);
        // Running and idle, no longer halted.
        assert_eq!(d.read32(MM2S_DMASR) & 3, SR_IDLE);
        wr(&mut d, MM2S_DMACR, 0);
        assert_eq!(d.read32(MM2S_DMASR) & 3, SR_HALTED);
        // Soft reset restores the reset values and self-clears.
        wr(&mut d, MM2S_DMACR, CR_RS | CR_IOC_EN | (5 << 16));
        wr(&mut d, S2MM_DMACR, CR_RESET);
        assert_eq!(d.read32(MM2S_DMACR), 0x0001_0002);
        assert_eq!(d.read32(S2MM_DMACR) & CR_RESET, 0);
    }

    #[test]
    fn simple_loopback_copy_with_interrupts() {
        let (mut d, h) = dma(&[("C_SG_LENGTH_WIDTH", "16")]);
        let mut m = Mem::new();
        for i in 0..256 {
            m.bytes[0x1000 + i] = i as u8 ^ 0x5A;
        }
        // S2MM first (the driver's order), then MM2S.
        wr(&mut d, S2MM_DMACR, CR_RS | CR_IOC_EN);
        wr(&mut d, S2MM_DA, 0x2000);
        wr(&mut d, S2MM_LENGTH, 256);
        wr(&mut d, MM2S_DMACR, CR_RS | CR_IOC_EN);
        wr(&mut d, MM2S_SA, 0x1000);
        wr(&mut d, MM2S_LENGTH, 256);
        assert_eq!(d.read32(MM2S_DMASR) & SR_IDLE, 0, "busy right after LENGTH");
        let mut t = 0;
        t = run(&mut d, &mut m, t, 100_000);
        let _ = t;
        assert_eq!(&m.bytes[0x2000..0x2100], &m.bytes[0x1000..0x1100]);
        assert_eq!(d.read32(S2MM_LENGTH), 256);
        assert_eq!(
            d.read32(MM2S_DMASR) & (SR_IDLE | SR_IOC_IRQ),
            SR_IDLE | SR_IOC_IRQ
        );
        assert_eq!(
            d.read32(S2MM_DMASR) & (SR_IDLE | SR_IOC_IRQ),
            SR_IDLE | SR_IOC_IRQ
        );
        assert!(d.irq_output(0) && d.irq_output(1));
        // Write 1 to clear.
        wr(&mut d, MM2S_DMASR, SR_IOC_IRQ);
        assert!(!d.irq_output(0) && d.irq_output(1));
        wr(&mut d, S2MM_DMASR, SR_IOC_IRQ);
        assert!(!d.irq_level() && !d.irq_output(1));
        let st = h.stats();
        assert_eq!(
            (st.mm2s_packets, st.mm2s_bytes, st.s2mm_bytes),
            (1, 256, 256)
        );
        // The master accesses are logged for the trace.
        let log = h.take_log();
        assert!(
            log.iter()
                .any(|a| !a.write && a.addr == 0x1000 && a.len == 256 && a.ok)
        );
        assert!(
            log.iter()
                .any(|a| a.write && a.addr == 0x2000 && a.len == 256)
        );
    }

    #[test]
    fn status_bits_follow_the_interrupt_enables() {
        let (mut d, _) = dma(&[]);
        let mut m = Mem::new();
        // IOC interrupt disabled: the status bit sets, the output stays low.
        wr(&mut d, S2MM_DMACR, CR_RS);
        wr(&mut d, S2MM_DA, 0x2000);
        wr(&mut d, S2MM_LENGTH, 64);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_SA, 0x1000);
        wr(&mut d, MM2S_LENGTH, 64);
        run(&mut d, &mut m, 0, 50_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_IOC_IRQ, 0);
        assert!(!d.irq_output(0));
        wr(&mut d, MM2S_DMACR, CR_RS | CR_IOC_EN);
        assert!(
            d.irq_output(0),
            "enabling the interrupt exposes the pending status"
        );
    }

    #[test]
    fn s2mm_waits_for_the_stream_and_a_short_packet_sets_length() {
        let (mut d, h) = dma(&[("STREAM_LOOPBACK", "0")]);
        let mut m = Mem::new();
        wr(&mut d, S2MM_DMACR, CR_RS | CR_IOC_EN);
        wr(&mut d, S2MM_DA, 0x3000);
        wr(&mut d, S2MM_LENGTH, 100);
        let t = run(&mut d, &mut m, 0, 20_000);
        assert_eq!(d.read32(S2MM_DMASR) & SR_IDLE, 0, "no data yet");
        h.push_s2mm((0..40).collect());
        run(&mut d, &mut m, t, 20_000);
        assert_eq!(d.read32(S2MM_LENGTH), 40, "TLAST ended the transfer early");
        assert_eq!(&m.bytes[0x3000..0x3028], &(0..40).collect::<Vec<u8>>()[..]);
        assert_ne!(d.read32(S2MM_DMASR) & SR_IDLE, 0);
    }

    #[test]
    fn long_packet_continues_in_the_next_transfer() {
        let (mut d, h) = dma(&[("STREAM_LOOPBACK", "0")]);
        let mut m = Mem::new();
        h.push_s2mm((0..30).collect());
        wr(&mut d, S2MM_DMACR, CR_RS);
        wr(&mut d, S2MM_DA, 0x3000);
        wr(&mut d, S2MM_LENGTH, 16);
        let t = run(&mut d, &mut m, 0, 20_000);
        assert_eq!(d.read32(S2MM_LENGTH), 16);
        wr(&mut d, S2MM_DA, 0x3100);
        wr(&mut d, S2MM_LENGTH, 64);
        run(&mut d, &mut m, t, 20_000);
        assert_eq!(d.read32(S2MM_LENGTH), 14, "the rest of the packet");
        assert_eq!(m.bytes[0x3100], 16);
    }

    #[test]
    fn mm2s_without_loopback_leaves_packets_for_the_host() {
        let (mut d, h) = dma(&[("STREAM_LOOPBACK", "0")]);
        let mut m = Mem::new();
        m.bytes[0x100..0x104].copy_from_slice(b"abcd");
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_SA, 0x100);
        wr(&mut d, MM2S_LENGTH, 4);
        run(&mut d, &mut m, 0, 20_000);
        assert_eq!(h.take_mm2s(), vec![b"abcd".to_vec()]);
        assert!(h.take_mm2s().is_empty());
    }

    #[test]
    fn decode_slave_and_internal_errors_halt_the_channel() {
        let (mut d, _) = dma(&[]);
        let mut m = Mem::new();
        m.fail_above = Some(0x8000);
        wr(&mut d, MM2S_DMACR, CR_RS | CR_ERR_EN);
        wr(&mut d, MM2S_SA, 0x9000);
        wr(&mut d, MM2S_LENGTH, 16);
        run(&mut d, &mut m, 0, 10_000);
        let sr = d.read32(MM2S_DMASR);
        assert_ne!(sr & SR_DMA_DEC_ERR, 0, "{sr:#x}");
        assert_ne!(sr & SR_ERR_IRQ, 0);
        assert_ne!(sr & SR_HALTED, 0);
        assert!(d.irq_output(0));
        // The error stays until a reset; clearing Err_Irq drops the interrupt only.
        wr(&mut d, MM2S_DMASR, SR_ERR_IRQ);
        assert!(!d.irq_output(0));
        assert_ne!(d.read32(MM2S_DMASR) & SR_DMA_DEC_ERR, 0);
        wr(&mut d, MM2S_DMACR, CR_RESET);
        assert_eq!(d.read32(MM2S_DMASR), 0x0001_0001);

        // Slave error.
        m.fail_above = None;
        m.slave_error_at = Some(0x40);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_SA, 0x40);
        wr(&mut d, MM2S_LENGTH, 8);
        run(&mut d, &mut m, 0, 10_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_DMA_SLV_ERR, 0);

        // Zero length: internal error.
        let (mut d2, _) = dma(&[]);
        wr(&mut d2, MM2S_DMACR, CR_RS);
        wr(&mut d2, MM2S_LENGTH, 0);
        assert_ne!(d2.read32(MM2S_DMASR) & SR_DMA_INT_ERR, 0);
    }

    #[test]
    fn length_is_limited_by_the_length_width_and_requires_run() {
        let (mut d, h) = dma(&[("C_SG_LENGTH_WIDTH", "8")]);
        let mut m = Mem::new();
        // Not running: LENGTH is ignored.
        wr(&mut d, MM2S_SA, 0x100);
        wr(&mut d, MM2S_LENGTH, 16);
        run(&mut d, &mut m, 0, 10_000);
        assert_eq!(h.stats().mm2s_bytes, 0);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_LENGTH, 0x1_10);
        run(&mut d, &mut m, 0, 10_000);
        assert_eq!(
            h.stats().mm2s_bytes,
            0x10,
            "bit 8 is beyond the 8-bit length"
        );
    }

    #[test]
    fn interrupt_threshold_and_delay() {
        let (mut d, _) = dma(&[("STREAM_LOOPBACK", "0")]);
        let mut m = Mem::new();
        // Threshold 2: the first completion does not interrupt, the second does.
        wr(&mut d, MM2S_DMACR, CR_RS | CR_IOC_EN | (2 << 16));
        wr(&mut d, MM2S_SA, 0x100);
        wr(&mut d, MM2S_LENGTH, 8);
        let t = run(&mut d, &mut m, 0, 10_000);
        assert_eq!(d.read32(MM2S_DMASR) & SR_IOC_IRQ, 0);
        assert_eq!(
            d.read32(MM2S_DMASR) >> 16 & 0xFF,
            1,
            "one more completion needed"
        );
        wr(&mut d, MM2S_LENGTH, 8);
        run(&mut d, &mut m, t, 10_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_IOC_IRQ, 0);

        // Delay timer: one completion below threshold 4 raises Dly_Irq after IRQDelay*125 clocks
        // (here 2 * 125 = 250 clocks = 2.5 us).
        let (mut d, _) = dma(&[("STREAM_LOOPBACK", "0")]);
        wr(
            &mut d,
            MM2S_DMACR,
            CR_RS | CR_DLY_EN | (4 << 16) | (2 << 24),
        );
        wr(&mut d, MM2S_SA, 0x100);
        wr(&mut d, MM2S_LENGTH, 4);
        run(&mut d, &mut m, 0, 2_000);
        assert_eq!(d.read32(MM2S_DMASR) & SR_DLY_IRQ, 0);
        run(&mut d, &mut m, 2_000, 6_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_DLY_IRQ, 0);
        assert!(d.irq_output(0));
    }

    /// Builds a descriptor at `at`.
    fn bd(m: &mut Mem, at: u64, next: u64, buf: u64, ctrl: u32) {
        m.put_word(at, next as u32);
        m.put_word(at + 8, buf as u32);
        m.put_word(at + 0x18, ctrl);
        m.put_word(at + 0x1C, 0);
    }

    #[test]
    fn scatter_gather_two_descriptor_packet_loopback() {
        let (mut d, h) = dma(&[("C_INCLUDE_SG", "1")]);
        let mut m = Mem::new();
        for i in 0..96 {
            m.bytes[0x1000 + i] = i as u8;
        }
        // MM2S: two BDs form one packet (SOF on the first, EOF on the second).
        bd(&mut m, 0x400, 0x440, 0x1000, BD_CTRL_SOF | 64);
        bd(&mut m, 0x440, 0x400, 0x1040, BD_CTRL_EOF | 32);
        // S2MM: two receive BDs of 50 bytes each.
        bd(&mut m, 0x500, 0x540, 0x3000, 50);
        bd(&mut m, 0x540, 0x500, 0x3100, 50);

        wr(&mut d, S2MM_CURDESC, 0x500);
        wr(&mut d, S2MM_DMACR, CR_RS | CR_IOC_EN);
        wr(&mut d, S2MM_TAILDESC, 0x540);
        wr(&mut d, MM2S_CURDESC, 0x400);
        wr(&mut d, MM2S_DMACR, CR_RS | CR_IOC_EN);
        assert_ne!(d.read32(MM2S_DMASR) & SR_SG_INCLD, 0);
        wr(&mut d, MM2S_TAILDESC, 0x440);
        run(&mut d, &mut m, 0, 100_000);

        assert_eq!(&m.bytes[0x3000..0x3032], &m.bytes[0x1000..0x1032]);
        assert_eq!(&m.bytes[0x3100..0x3100 + 46], &m.bytes[0x1032..0x1032 + 46]);
        // Descriptor write-back.
        assert_eq!(m.word(0x41C), BD_STS_CMPLT | 64, "MM2S BD 1");
        assert_eq!(m.word(0x45C), BD_STS_CMPLT | 32, "MM2S BD 2");
        assert_eq!(
            m.word(0x51C),
            BD_STS_CMPLT | BD_STS_SOF | 50,
            "S2MM BD 1: first of packet"
        );
        assert_eq!(
            m.word(0x55C),
            BD_STS_CMPLT | BD_STS_EOF | 46,
            "S2MM BD 2: last of packet"
        );
        assert_ne!(d.read32(MM2S_DMASR) & SR_IDLE, 0);
        assert_ne!(d.read32(S2MM_DMASR) & SR_IDLE, 0);
        assert_eq!(h.stats().descriptors, 4);
        assert!(d.irq_output(0) && d.irq_output(1));
        // Appending: move the tail on to the (already used) BD chain would raise SGIntErr,
        // so use a fresh BD.
        bd(&mut m, 0x580, 0x500, 0x3200, 50);
        m.put_word(0x540, 0x580);
        h.push_s2mm(vec![9; 20]);
        wr(&mut d, S2MM_TAILDESC, 0x580);
        run(&mut d, &mut m, 100_000, 50_000);
        assert_eq!(m.word(0x59C), BD_STS_CMPLT | BD_STS_SOF | BD_STS_EOF | 20);
    }

    #[test]
    fn sg_descriptor_errors() {
        let (mut d, _) = dma(&[("C_INCLUDE_SG", "1")]);
        let mut m = Mem::new();
        // A completed descriptor fetched again: SGIntErr.
        bd(&mut m, 0x400, 0x400, 0x1000, BD_CTRL_SOF | BD_CTRL_EOF | 8);
        m.put_word(0x41C, BD_STS_CMPLT);
        wr(&mut d, MM2S_CURDESC, 0x400);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_TAILDESC, 0x400);
        run(&mut d, &mut m, 0, 10_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_SG_INT_ERR, 0);

        // A descriptor outside memory: SGDecErr.
        let (mut d, _) = dma(&[("C_INCLUDE_SG", "1")]);
        m.fail_above = Some(0x8000);
        wr(&mut d, MM2S_CURDESC, 0x9000);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_TAILDESC, 0x9000);
        run(&mut d, &mut m, 0, 10_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_SG_DEC_ERR, 0);

        // A buffer outside memory: DMADecErr, reported in the descriptor too.
        let (mut d, _) = dma(&[("C_INCLUDE_SG", "1")]);
        bd(&mut m, 0x400, 0x400, 0x9000, BD_CTRL_SOF | BD_CTRL_EOF | 8);
        wr(&mut d, MM2S_CURDESC, 0x400);
        wr(&mut d, MM2S_DMACR, CR_RS);
        wr(&mut d, MM2S_TAILDESC, 0x400);
        run(&mut d, &mut m, 0, 10_000);
        assert_ne!(d.read32(MM2S_DMASR) & SR_DMA_DEC_ERR, 0);
        assert_eq!(m.word(0x41C) & BD_STS_DEC_ERR, BD_STS_DEC_ERR);
    }

    #[test]
    fn registers_of_the_wrong_mode_read_zero() {
        let (mut d, _) = dma(&[]);
        wr(&mut d, MM2S_CURDESC, 0x1234);
        assert_eq!(
            d.read32(MM2S_CURDESC),
            0,
            "no SG registers on a simple core"
        );
        let (mut g, _) = dma(&[("C_INCLUDE_SG", "1")]);
        wr(&mut g, MM2S_SA, 0x1234);
        assert_eq!(g.read32(MM2S_SA), 0, "no SA register on an SG core");
        assert_eq!(g.read32(0x5C), 0);
        let (mut one, _) = dma(&[("C_INCLUDE_S2MM", "0")]);
        wr(&mut one, S2MM_DMACR, CR_RS);
        assert_eq!(one.read32(S2MM_DMACR), 0);
    }
}
