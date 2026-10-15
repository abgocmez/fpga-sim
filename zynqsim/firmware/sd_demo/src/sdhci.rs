//! A polled SD Host Controller driver for the Zynq-7000's SDIO0 (the Arasan
//! SDHCI core, standard register map), plus the SD card bring-up the FSBL's
//! `xilffs` would do. One 512-byte block per command, PIO, 1-bit bus, no
//! interrupts and no DMA: the smallest thing that can mount a FAT volume.
//!
//! Every wait is bounded, so a model gap fails fast instead of hanging.
//!
//! VERIFY: register offsets and bit positions are written from memory of the
//! SD Host Controller Simplified Specification v3.00 and the Zynq TRM (UG585,
//! ch. 13); they are exercised against QEMU's SDHCI model, not checked on
//! hardware. High-speed mode, 4-bit bus, DMA and card-detect interrupts are not
//! implemented.

use core::cell::Cell;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};

/// SDIO0 register block (PS peripheral, `sdhci@e0100000` in the device tree).
pub const SDIO0_BASE: usize = 0xE010_0000;

// Register offsets (SD Host Controller spec).
const BLK_SIZE: usize = 0x04; // 16 bit
const BLK_COUNT: usize = 0x06; // 16 bit
const ARGUMENT: usize = 0x08; // 32 bit
const XFER_MODE: usize = 0x0C; // 16 bit (CMD is the upper half of the word)
const RESPONSE: usize = 0x10; // 4 x 32 bit
const BUFFER: usize = 0x20; // 32 bit data port
const PRESENT: usize = 0x24; // 32 bit
const POWER: usize = 0x29; // 8 bit
const CLOCK: usize = 0x2C; // 16 bit
const TIMEOUT: usize = 0x2E; // 8 bit
const SW_RESET: usize = 0x2F; // 8 bit
const INT_STATUS: usize = 0x30; // 16 bit
const ERR_STATUS: usize = 0x32; // 16 bit
const INT_ENABLE: usize = 0x34; // 16 bit
const ERR_ENABLE: usize = 0x36; // 16 bit
const SIGNAL_ENABLE: usize = 0x38; // 16 bit

// PRESENT bits.
const PS_CMD_INHIBIT: u32 = 1 << 0;
const PS_DAT_INHIBIT: u32 = 1 << 1;
const PS_CARD_INSERTED: u32 = 1 << 16;

// INT_STATUS bits.
const INT_CMD_COMPLETE: u16 = 1 << 0;
const INT_XFER_COMPLETE: u16 = 1 << 1;
const INT_BUF_WRITE_READY: u16 = 1 << 4;
const INT_BUF_READ_READY: u16 = 1 << 5;
const INT_ERROR: u16 = 1 << 15;

// Command register: response type and checks.
const RSP_NONE: u16 = 0x00;
const RSP_R2: u16 = 0x09; // 136 bit, CRC check
const RSP_R3: u16 = 0x02; // 48 bit, no CRC / index check
const RSP_R1: u16 = 0x1A; // 48 bit, CRC and index check
const RSP_R1B: u16 = 0x1B; // 48 bit with busy
const CMD_DATA: u16 = 0x20;

/// Why an SD operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdError {
    /// The controller reports no card in the slot.
    NoCard,
    /// A wait ran out; the text says for what.
    Timeout(&'static str),
    /// The controller flagged an error for a command: index and error
    /// interrupt status bits.
    Command(u8, u16),
    /// The card answered, but not as the protocol says.
    Protocol(&'static str),
}

impl fmt::Display for SdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SdError::NoCard => write!(f, "no card in the slot"),
            SdError::Timeout(what) => write!(f, "timeout waiting for {what}"),
            SdError::Command(i, e) => write!(f, "CMD{i} failed, error status {e:#06x}"),
            SdError::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl core::error::Error for SdError {}

#[inline]
fn r8(off: usize) -> u8 {
    // SAFETY: SDIO0 is a device register block on the simulated bus.
    unsafe { read_volatile((SDIO0_BASE + off) as *const u8) }
}
#[inline]
fn w8(off: usize, v: u8) {
    // SAFETY: as in `r8`.
    unsafe { write_volatile((SDIO0_BASE + off) as *mut u8, v) }
}
#[inline]
fn r16(off: usize) -> u16 {
    // SAFETY: as in `r8`.
    unsafe { read_volatile((SDIO0_BASE + off) as *const u16) }
}
#[inline]
fn w16(off: usize, v: u16) {
    // SAFETY: as in `r8`.
    unsafe { write_volatile((SDIO0_BASE + off) as *mut u16, v) }
}
#[inline]
fn r32(off: usize) -> u32 {
    rt::read32(SDIO0_BASE + off)
}
#[inline]
fn w32(off: usize, v: u32) {
    rt::write32(SDIO0_BASE + off, v)
}

/// Polls `done` up to `tries` times, `pace_us` microseconds apart.
fn wait(what: &'static str, tries: u32, pace_us: u32, mut done: impl FnMut() -> bool) -> Result<(), SdError> {
    for _ in 0..tries {
        if done() {
            return Ok(());
        }
        rt::delay_us(pace_us);
    }
    Err(SdError::Timeout(what))
}

/// An initialised card on SDIO0.
pub struct Card {
    /// Relative card address from CMD3 (shifted into the argument position).
    rca: Cell<u32>,
    /// OCR bit 30: block addressing (SDHC/SDXC) instead of byte addressing.
    high_capacity: Cell<bool>,
    /// Capacity in 512-byte blocks, from the CSD.
    blocks: Cell<u32>,
    /// The 128-bit CID (CRC byte stripped by the controller).
    cid: Cell<[u32; 4]>,
}

/// What `Card::init` learned.
#[derive(Clone, Copy, Debug)]
pub struct CardInfo {
    /// Block addressing (SDHC/SDXC) instead of byte addressing (SDSC).
    pub high_capacity: bool,
    /// Capacity in 512-byte blocks.
    pub blocks: u32,
    /// Relative card address.
    pub rca: u16,
    /// OCR as read after ACMD41.
    pub ocr: u32,
    /// Product name from the CID, five ASCII characters.
    pub product: [u8; 5],
    /// Manufacturer id from the CID.
    pub manufacturer: u8,
}

impl Card {
    /// A card that has not been initialised yet.
    pub const fn new() -> Card {
        Card {
            rca: Cell::new(0),
            high_capacity: Cell::new(false),
            blocks: Cell::new(0),
            cid: Cell::new([0; 4]),
        }
    }

    fn command(
        &self,
        index: u8,
        arg: u32,
        rsp: u16,
        data: bool,
        read: bool,
    ) -> Result<[u32; 4], SdError> {
        let busy_wait = if data || rsp == RSP_R1B { PS_DAT_INHIBIT } else { 0 };
        wait("command inhibit to clear", 2000, 5, || {
            r32(PRESENT) & (PS_CMD_INHIBIT | busy_wait) == 0
        })?;
        // Clear stale interrupt status.
        w16(INT_STATUS, 0xFFFF);
        w16(ERR_STATUS, 0xFFFF);
        w32(ARGUMENT, arg);
        // Transfer mode: bit 4 = read direction. Single block, no DMA.
        let mode: u16 = if data && read { 0x0010 } else { 0 };
        let cmd: u16 = (u16::from(index) << 8) | rsp | if data { CMD_DATA } else { 0 };
        // One 32-bit write of {command, transfer mode}: the write that issues
        // the command, as Linux's sdhci does.
        w32(XFER_MODE, (u32::from(cmd) << 16) | u32::from(mode));
        let mut status = 0u16;
        let r = wait("command complete", 4000, 5, || {
            status = r16(INT_STATUS);
            status & (INT_CMD_COMPLETE | INT_ERROR) != 0
        });
        if r.is_err() || status & INT_ERROR != 0 {
            let err = r16(ERR_STATUS);
            // Reset the command and data lines before anyone retries.
            w8(SW_RESET, 0x06);
            let _ = wait("reset", 100, 5, || r8(SW_RESET) & 0x06 == 0);
            w16(INT_STATUS, 0xFFFF);
            w16(ERR_STATUS, 0xFFFF);
            return match r {
                Err(e) if err == 0 => Err(e),
                _ => Err(SdError::Command(index, err)),
            };
        }
        w16(INT_STATUS, INT_CMD_COMPLETE);
        let mut out = [0u32; 4];
        if rsp != RSP_NONE {
            out[0] = r32(RESPONSE);
            if rsp == RSP_R2 {
                out[1] = r32(RESPONSE + 4);
                out[2] = r32(RESPONSE + 8);
                out[3] = r32(RESPONSE + 12);
            }
        }
        if rsp == RSP_R1B && !data {
            // Busy signalling completes with "transfer complete".
            wait("busy to clear", 2000, 5, || {
                let s = r16(INT_STATUS);
                s & INT_XFER_COMPLETE != 0
            })?;
            w16(INT_STATUS, INT_XFER_COMPLETE);
        }
        Ok(out)
    }

    fn app_command(&self, index: u8, arg: u32, rsp: u16) -> Result<[u32; 4], SdError> {
        self.command(55, self.rca.get(), RSP_R1, false, false)?;
        self.command(index, arg, rsp, false, false)
    }

    /// Brings the controller and the card up: reset, power, clock, CMD0/8,
    /// ACMD41, CMD2/3/9/7/16.
    pub fn init(&self) -> Result<CardInfo, SdError> {
        w8(SW_RESET, 0x01);
        wait("controller reset", 1000, 5, || r8(SW_RESET) & 0x01 == 0)?;
        if r32(PRESENT) & PS_CARD_INSERTED == 0 {
            return Err(SdError::NoCard);
        }
        // 3.3 V, power on.
        w8(POWER, 0x0F);
        // Internal clock on, wait for it to be stable, then clock the card
        // (divider 0x80: identification-mode frequency).
        w16(CLOCK, 0x8000 | 0x0001);
        wait("internal clock stable", 1000, 5, || r16(CLOCK) & 0x0002 != 0)?;
        w16(CLOCK, 0x8000 | 0x0004 | 0x0001);
        w8(TIMEOUT, 0x0E);
        w16(INT_ENABLE, 0x01FF);
        w16(ERR_ENABLE, 0x01FF);
        w16(SIGNAL_ENABLE, 0);
        w16(BLK_SIZE, 512);
        w16(BLK_COUNT, 1);
        self.rca.set(0);

        self.command(0, 0, RSP_NONE, false, false)?;
        // CMD8: 2.7-3.6 V, check pattern 0xAA. A v2.0+ card echoes it.
        let r7 = self.command(8, 0x0000_01AA, RSP_R1, false, false)?;
        if r7[0] & 0xFFF != 0x1AA {
            return Err(SdError::Protocol("CMD8 did not echo the check pattern"));
        }
        // ACMD41 until the card leaves its power-up routine (OCR bit 31).
        let mut ocr = 0u32;
        let mut ready = false;
        for _ in 0..400 {
            let r3 = self.app_command(41, 0x40FF_8000, RSP_R3)?;
            ocr = r3[0];
            if ocr & (1 << 31) != 0 {
                ready = true;
                break;
            }
            rt::delay_us(500);
        }
        if !ready {
            return Err(SdError::Timeout("the card to power up (ACMD41)"));
        }
        self.high_capacity.set(ocr & (1 << 30) != 0);

        let cid = self.command(2, 0, RSP_R2, false, false)?;
        self.cid.set(cid);
        let r6 = self.command(3, 0, RSP_R1, false, false)?;
        let rca = r6[0] & 0xFFFF_0000;
        if rca == 0 {
            return Err(SdError::Protocol("the card reported RCA 0"));
        }
        self.rca.set(rca);
        let csd = self.command(9, rca, RSP_R2, false, false)?;
        self.blocks.set(csd_blocks(&csd)?);
        self.command(7, rca, RSP_R1B, false, false)?;
        if !self.high_capacity.get() {
            // Standard-capacity cards: make the block length explicit.
            self.command(16, 512, RSP_R1, false, false)?;
        }
        // The controller strips the CRC byte of a 136-bit response, so
        // register bit n is CID bit n + 8: the manufacturer id (CID[127:120])
        // is cid[3][23:16] and the product name (CID[103:64], five
        // characters) is register bits [95:56].
        let product = [
            ((cid[2] >> 24) & 0xFF) as u8,
            ((cid[2] >> 16) & 0xFF) as u8,
            ((cid[2] >> 8) & 0xFF) as u8,
            (cid[2] & 0xFF) as u8,
            ((cid[1] >> 24) & 0xFF) as u8,
        ];
        Ok(CardInfo {
            high_capacity: self.high_capacity.get(),
            blocks: self.blocks.get(),
            rca: (rca >> 16) as u16,
            ocr,
            product,
            manufacturer: ((cid[3] >> 16) & 0xFF) as u8,
        })
    }

    fn addr(&self, lba: u32) -> u32 {
        if self.high_capacity.get() {
            lba
        } else {
            lba.wrapping_mul(512)
        }
    }

    /// CMD17: reads one block by PIO.
    pub fn read_block(&self, lba: u32, out: &mut [u8; 512]) -> Result<(), SdError> {
        w16(BLK_SIZE, 512);
        w16(BLK_COUNT, 1);
        self.command(17, self.addr(lba), RSP_R1, true, true)?;
        wait("read buffer ready", 4000, 5, || {
            r16(INT_STATUS) & (INT_BUF_READ_READY | INT_ERROR) != 0
        })?;
        if r16(INT_STATUS) & INT_ERROR != 0 {
            return Err(SdError::Command(17, r16(ERR_STATUS)));
        }
        for chunk in out.chunks_exact_mut(4) {
            chunk.copy_from_slice(&r32(BUFFER).to_le_bytes());
        }
        wait("read transfer complete", 4000, 5, || {
            r16(INT_STATUS) & (INT_XFER_COMPLETE | INT_ERROR) != 0
        })?;
        w16(INT_STATUS, 0xFFFF);
        Ok(())
    }

    /// CMD24: writes one block by PIO.
    pub fn write_block(&self, lba: u32, data: &[u8; 512]) -> Result<(), SdError> {
        w16(BLK_SIZE, 512);
        w16(BLK_COUNT, 1);
        self.command(24, self.addr(lba), RSP_R1, true, false)?;
        wait("write buffer ready", 4000, 5, || {
            r16(INT_STATUS) & (INT_BUF_WRITE_READY | INT_ERROR) != 0
        })?;
        if r16(INT_STATUS) & INT_ERROR != 0 {
            return Err(SdError::Command(24, r16(ERR_STATUS)));
        }
        for chunk in data.chunks_exact(4) {
            w32(BUFFER, u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        wait("write transfer complete", 4000, 5, || {
            r16(INT_STATUS) & (INT_XFER_COMPLETE | INT_ERROR) != 0
        })?;
        w16(INT_STATUS, 0xFFFF);
        Ok(())
    }
}

/// Capacity in 512-byte blocks from a CSD register (R2 response, CRC byte
/// stripped, so register bit `n` is CSD bit `n + 8`).
fn csd_blocks(r: &[u32; 4]) -> Result<u32, SdError> {
    // Bit field `[hi:lo]` of the 120-bit response value.
    let field = |hi: u32, lo: u32| -> u32 {
        let mut v = 0u64;
        for bit in (lo..=hi).rev() {
            let w = r[(bit / 32) as usize];
            v = (v << 1) | u64::from((w >> (bit % 32)) & 1);
        }
        v as u32
    };
    match field(119, 118) {
        // CSD v1.0 (standard capacity): C_SIZE [73:62], C_SIZE_MULT [49:47],
        // READ_BL_LEN [83:80]; shifted down by 8.
        0 => {
            let c_size = field(65, 54);
            let mult = field(41, 39);
            let read_bl_len = field(75, 72);
            let blocks = u64::from(c_size + 1) << (mult + 2);
            let bytes = blocks << read_bl_len;
            u32::try_from(bytes / 512).map_err(|_| SdError::Protocol("capacity overflows"))
        }
        // CSD v2.0 (SDHC/SDXC): C_SIZE [69:48]: (C_SIZE + 1) * 512 KiB.
        1 => {
            let c_size = field(61, 40);
            Ok((c_size + 1) * 1024)
        }
        _ => Err(SdError::Protocol("unknown CSD structure version")),
    }
}

/// The card as an `embedded-sdmmc` block device.
pub struct SdBlockDevice<'a>(pub &'a Card);

impl BlockDevice for SdBlockDevice<'_> {
    type Error = SdError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), SdError> {
        for (i, b) in blocks.iter_mut().enumerate() {
            self.0.read_block(start.0 + i as u32, &mut b.contents)?;
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), SdError> {
        for (i, b) in blocks.iter().enumerate() {
            self.0.write_block(start.0 + i as u32, &b.contents)?;
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, SdError> {
        Ok(BlockCount(self.0.blocks.get()))
    }
}
