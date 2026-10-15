//! Driver for the Cadence GEM Gigabit Ethernet MAC of the Zynq-7000 PS
//! (GEM0 at `0xE000_B000`): MDIO access to the PHY, descriptor rings in DDR,
//! polled receive and transmit, and a `smoltcp` [`Device`] on top.
//!
//! Register offsets and descriptor layouts are those of the Xilinx
//! `xemacps_hw.h` / `xemacps_bd.h` standalone driver and UG585 (GEM chapter).
//! VERIFY: written from memory of those files, not re-checked against UG585;
//! what the simulator proves is that QEMU's `cadence_gem` model accepts them.
//!
//! The firmware runs on one core, with the MMU and caches off and no
//! interrupts, so the rings are plain statics and the DMA is coherent.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use rt::{read32, write32};
use smoltcp::phy::{Checksum, ChecksumCapabilities, Device, DeviceCapabilities, Medium};
use smoltcp::phy::{RxToken, TxToken};
use smoltcp::time::Instant;

/// GEM0 register block.
pub const GEM0: usize = 0xE000_B000;

const NWCTRL: usize = 0x000;
const NWCFG: usize = 0x004;
const NWSR: usize = 0x008;
const DMACR: usize = 0x010;
const TXSR: usize = 0x014;
const RXQBASE: usize = 0x018;
const TXQBASE: usize = 0x01C;
const RXSR: usize = 0x020;
const ISR: usize = 0x024;
const IDR: usize = 0x02C;
const PHYMNTNC: usize = 0x034;
const SPADDR1L: usize = 0x088;
const SPADDR1H: usize = 0x08C;

const NWCTRL_RXEN: u32 = 1 << 2;
const NWCTRL_TXEN: u32 = 1 << 3;
const NWCTRL_MDEN: u32 = 1 << 4;
const NWCTRL_STARTTX: u32 = 1 << 9;

const NWCFG_SPEED100: u32 = 1 << 0;
const NWCFG_FDPLX: u32 = 1 << 1;
const NWCFG_RX1536: u32 = 1 << 8;
const NWCFG_FCSREM: u32 = 1 << 17;
/// MDC = pclk / 48 (the field value is a code, not a divisor).
const NWCFG_MDCDIV48: u32 = 3 << 18;

const NWSR_MDIO_IDLE: u32 = 1 << 2;

/// INCR16 bursts, 8 KiB RX / 4 KiB TX packet buffers, 1536-byte RX buffers.
const DMACR_VALUE: u32 = 0x10 | (3 << 8) | (1 << 10) | (24 << 16);

/// RX descriptor word 0: set by the MAC when a frame was stored.
const RX_NEW: u32 = 1 << 0;
const RX_WRAP: u32 = 1 << 1;
const RX_LEN_MASK: u32 = 0x1FFF;
/// TX descriptor word 1.
const TX_USED: u32 = 1 << 31;
const TX_WRAP: u32 = 1 << 30;
const TX_LAST: u32 = 1 << 15;

/// PHY (IEEE 802.3 clause 22) registers.
const MII_BMCR: u8 = 0;
const MII_BMSR: u8 = 1;
const MII_PHYSID1: u8 = 2;
const MII_PHYSID2: u8 = 3;
const BMCR_RESET: u16 = 1 << 15;
const BMCR_ANENABLE: u16 = 1 << 12;
const BMCR_ANRESTART: u16 = 1 << 9;
const BMSR_LSTATUS: u16 = 1 << 2;
const BMSR_ANEGCOMPLETE: u16 = 1 << 5;

/// Receive buffers and descriptors in the ring.
const RX_SLOTS: usize = 8;
const TX_SLOTS: usize = 4;
const BUF_SIZE: usize = 1536;

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct Desc {
    addr: u32,
    status: u32,
}

#[repr(C, align(32))]
struct Buf([u8; BUF_SIZE]);

/// Everything the MAC reads and writes by DMA.
struct Dma {
    rx_desc: [Desc; RX_SLOTS],
    tx_desc: [Desc; TX_SLOTS],
    rx_buf: [Buf; RX_SLOTS],
    tx_buf: [Buf; TX_SLOTS],
}

struct Shared(UnsafeCell<Dma>);

// SAFETY: one core, no interrupts, and the only user is the single `Gem`.
unsafe impl Sync for Shared {}

static DMA: Shared = Shared(UnsafeCell::new(Dma {
    rx_desc: [Desc { addr: 0, status: 0 }; RX_SLOTS],
    tx_desc: [Desc { addr: 0, status: 0 }; TX_SLOTS],
    rx_buf: [const { Buf([0; BUF_SIZE]) }; RX_SLOTS],
    tx_buf: [const { Buf([0; BUF_SIZE]) }; TX_SLOTS],
}));

fn dma() -> *mut Dma {
    DMA.0.get()
}

/// Reads a descriptor word the MAC may have written (volatile).
fn rx_status(i: usize) -> u32 {
    // SAFETY: `i < RX_SLOTS`; the descriptor lives in a static.
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*dma()).rx_desc[i].addr)) }
}

fn rx_len_word(i: usize) -> u32 {
    // SAFETY: as in `rx_status`.
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*dma()).rx_desc[i].status)) }
}

fn rx_release(i: usize) {
    // SAFETY: as in `rx_status`. Clearing NEW hands the slot back to the MAC.
    unsafe {
        let p = core::ptr::addr_of_mut!((*dma()).rx_desc[i].addr);
        core::ptr::write_volatile(p, core::ptr::read_volatile(p) & !RX_NEW);
    }
}

fn tx_status(i: usize) -> u32 {
    // SAFETY: `i < TX_SLOTS`.
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*dma()).tx_desc[i].status)) }
}

/// Counters the firmware reports at the end.
#[derive(Clone, Copy, Default)]
pub struct Counters {
    /// Frames handed to the MAC.
    pub tx_frames: u32,
    /// Frames taken from the receive ring.
    pub rx_frames: u32,
}

/// The MAC with its two rings.
pub struct Gem {
    rx_idx: usize,
    tx_idx: usize,
    /// Frame counters.
    pub counters: Counters,
}

/// Reads a PHY register over MDIO, or `None` on timeout.
fn mdio_read(phy: u8, reg: u8) -> Option<u16> {
    wait_mdio_idle()?;
    write32(
        GEM0 + PHYMNTNC,
        0x4002_0000 | 0x2000_0000 | (u32::from(phy) << 23) | (u32::from(reg) << 18),
    );
    wait_mdio_idle()?;
    Some((read32(GEM0 + PHYMNTNC) & 0xFFFF) as u16)
}

/// Writes a PHY register over MDIO.
fn mdio_write(phy: u8, reg: u8, value: u16) -> Option<()> {
    wait_mdio_idle()?;
    write32(
        GEM0 + PHYMNTNC,
        0x4002_0000
            | 0x1000_0000
            | (u32::from(phy) << 23)
            | (u32::from(reg) << 18)
            | u32::from(value),
    );
    wait_mdio_idle()
}

fn wait_mdio_idle() -> Option<()> {
    for _ in 0..1000 {
        if read32(GEM0 + NWSR) & NWSR_MDIO_IDLE != 0 {
            return Some(());
        }
        rt::delay_us(1);
    }
    None
}

/// What the PHY scan and link negotiation found.
pub struct PhyInfo {
    /// MDIO address of the PHY.
    pub addr: u8,
    /// PHY identifier registers 2 and 3.
    pub id: (u16, u16),
    /// Link up and auto-negotiation complete.
    pub link: bool,
}

/// Scans MDIO addresses 0..32 for a PHY (a register 2 that is neither all
/// zeros nor all ones, as the Xilinx lwIP port does), resets it and waits for
/// auto-negotiation.
pub fn bring_up_phy() -> Option<PhyInfo> {
    write32(GEM0 + NWCFG, NWCFG_MDCDIV48);
    write32(GEM0 + NWCTRL, NWCTRL_MDEN);
    let mut found = None;
    for addr in 0..32u8 {
        if let Some(id1) = mdio_read(addr, MII_PHYSID1).filter(|&v| v != 0xFFFF && v != 0) {
            found = Some((addr, id1, mdio_read(addr, MII_PHYSID2)?));
            break;
        }
    }
    let (addr, id1, id2) = found?;
    mdio_write(addr, MII_BMCR, BMCR_RESET)?;
    for _ in 0..1000 {
        if mdio_read(addr, MII_BMCR)? & BMCR_RESET == 0 {
            break;
        }
        rt::delay_us(10);
    }
    mdio_write(addr, MII_BMCR, BMCR_ANENABLE | BMCR_ANRESTART)?;
    let mut link = false;
    for _ in 0..2000 {
        let s = mdio_read(addr, MII_BMSR)?;
        if s & BMSR_LSTATUS != 0 && s & BMSR_ANEGCOMPLETE != 0 {
            link = true;
            break;
        }
        rt::delay_us(20);
    }
    Some(PhyInfo {
        addr,
        id: (id1, id2),
        link,
    })
}

impl Gem {
    /// Programs the rings, the MAC address and the receive filter, then
    /// enables the receiver and transmitter.
    pub fn new(mac: [u8; 6]) -> Gem {
        // SAFETY: nothing else touches the static before the MAC is enabled.
        let d = unsafe { &mut *dma() };
        for i in 0..RX_SLOTS {
            let wrap = if i == RX_SLOTS - 1 { RX_WRAP } else { 0 };
            d.rx_desc[i] = Desc {
                addr: (d.rx_buf[i].0.as_ptr() as u32 & !3) | wrap,
                status: 0,
            };
        }
        for i in 0..TX_SLOTS {
            let wrap = if i == TX_SLOTS - 1 { TX_WRAP } else { 0 };
            d.tx_desc[i] = Desc {
                addr: d.tx_buf[i].0.as_ptr() as u32,
                status: TX_USED | wrap,
            };
        }
        // Quiesce, clear statistics and status, mask interrupts (polled).
        write32(GEM0 + NWCTRL, NWCTRL_MDEN);
        write32(GEM0 + TXSR, 0xFFFF_FFFF);
        write32(GEM0 + RXSR, 0xFFFF_FFFF);
        write32(GEM0 + ISR, 0xFFFF_FFFF);
        write32(GEM0 + IDR, 0xFFFF_FFFF);
        write32(GEM0 + RXQBASE, d.rx_desc.as_ptr() as u32);
        write32(GEM0 + TXQBASE, d.tx_desc.as_ptr() as u32);
        write32(GEM0 + DMACR, DMACR_VALUE);
        write32(
            GEM0 + NWCFG,
            NWCFG_SPEED100 | NWCFG_FDPLX | NWCFG_RX1536 | NWCFG_FCSREM | NWCFG_MDCDIV48,
        );
        let lo = u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]);
        let hi = u32::from(mac[4]) | (u32::from(mac[5]) << 8);
        write32(GEM0 + SPADDR1L, lo);
        write32(GEM0 + SPADDR1H, hi);
        write32(GEM0 + NWCTRL, NWCTRL_MDEN | NWCTRL_RXEN | NWCTRL_TXEN);
        Gem {
            rx_idx: 0,
            tx_idx: 0,
            counters: Counters::default(),
        }
    }
}

/// A received frame waiting in slot `idx`.
pub struct GemRx<'a> {
    idx: usize,
    len: usize,
    _gem: PhantomData<&'a mut Gem>,
}

/// A free transmit slot.
pub struct GemTx<'a> {
    gem: *mut Gem,
    _gem: PhantomData<&'a mut Gem>,
}

impl RxToken for GemRx<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        // SAFETY: the slot belongs to software until released below.
        let frame = unsafe {
            let p = core::ptr::addr_of!((*dma()).rx_buf[self.idx].0) as *const u8;
            core::slice::from_raw_parts(p, self.len)
        };
        let r = f(frame);
        rx_release(self.idx);
        r
    }
}

impl TxToken for GemTx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let len = len.min(BUF_SIZE);
        // SAFETY: `gem` was made from the `&mut Gem` the token borrows. The
        // slot is software-owned (`USED` set) until the MAC is kicked below.
        let (r, idx) = unsafe {
            let idx = (*self.gem).tx_idx;
            let p = core::ptr::addr_of_mut!((*dma()).tx_buf[idx].0) as *mut u8;
            (f(core::slice::from_raw_parts_mut(p, len)), idx)
        };
        let wrap = if idx == TX_SLOTS - 1 { TX_WRAP } else { 0 };
        // SAFETY: as above; clearing USED gives the descriptor to the MAC.
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*dma()).tx_desc[idx].status),
                len as u32 | TX_LAST | wrap,
            );
            (*self.gem).tx_idx = (idx + 1) % TX_SLOTS;
            (*self.gem).counters.tx_frames += 1;
        }
        write32(
            GEM0 + NWCTRL,
            NWCTRL_MDEN | NWCTRL_RXEN | NWCTRL_TXEN | NWCTRL_STARTTX,
        );
        r
    }
}

impl Device for Gem {
    type RxToken<'a> = GemRx<'a>;
    type TxToken<'a> = GemTx<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(GemRx<'_>, GemTx<'_>)> {
        let i = self.rx_idx;
        if rx_status(i) & RX_NEW == 0 {
            return None;
        }
        // A transmit slot is needed for the reply smoltcp may generate.
        if tx_status(self.tx_idx) & TX_USED == 0 {
            return None;
        }
        let len = (rx_len_word(i) & RX_LEN_MASK) as usize;
        self.rx_idx = (i + 1) % RX_SLOTS;
        self.counters.rx_frames += 1;
        write32(GEM0 + RXSR, 0xF);
        let gem: *mut Gem = self;
        Some((
            GemRx {
                idx: i,
                len: len.min(BUF_SIZE),
                _gem: PhantomData,
            },
            GemTx {
                gem,
                _gem: PhantomData,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<GemTx<'_>> {
        if tx_status(self.tx_idx) & TX_USED == 0 {
            return None;
        }
        let gem: *mut Gem = self;
        Some(GemTx {
            gem,
            _gem: PhantomData,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        // The MAC's checksum offload is off: smoltcp computes and checks all.
        caps.checksum = ChecksumCapabilities::default();
        caps.checksum.ipv4 = Checksum::Both;
        caps.checksum.icmpv4 = Checksum::Both;
        caps
    }
}
