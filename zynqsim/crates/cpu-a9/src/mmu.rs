//! The ARMv7-A virtual memory system, short-descriptor format (ARM ARM B3.5 to B3.7), as the
//! Cortex-A9 uses it: TTBR0/TTBR1 split by TTBCR.N, domains from DACR, first-level sections
//! (1 MiB) and supersections (16 MiB), coarse tables with large (64 KiB) and small (4 KiB) pages,
//! AP[2:0]/APX permissions with XN and PXN, and a small software TLB.
//!
//! Scope and choices (see `docs/adr/030-cpu-a9.md`):
//!
//! * TEX, C and B are recorded in the [`Mapping`]. Caches are not simulated, so they change
//!   nothing. TEX remapping (SCTLR.TRE) is not modelled; the raw TEX/C/B are kept.
//! * The access flag is off (SCTLR.AFE is treated as 0): there are no access-flag faults.
//! * nG and the ASID (CONTEXTIDR[7:0]) only tag TLB entries. Nothing else depends on them.
//! * SCTLR.XP = 1 (the reset value) selects the AP[2:0] table; XP = 0 selects the ARMv5 subpage
//!   AP fields of small and large pages. The S and R bits (ARMv5 system protection) are taken
//!   as 0.
//! * A Manager domain (0b11) is not checked against AP, XN or PXN. A reserved domain value
//!   (0b10) is treated as No access. AP[2:0] = 0b100 is UNPREDICTABLE and is treated as No
//!   access. AP[2:0] = 0b110 and 0b111 are read-only for both modes.
//! * The TLB caches the descriptor fields, not the permission result: the domain and AP checks
//!   run on every access with the current DACR and SCTLR. Changing a table is not seen until a
//!   TLB invalidate (TLBIALL, TLBIMVA, TLBIMVAA, TLBIASID): as on hardware. Faults are never
//!   cached. Changing TTBR0, TTBR1, TTBCR, CONTEXTIDR or SCTLR.M does not flush the TLB either.

use ps_bus::Bus;

use crate::cp15::SCTLR_M;
use crate::state::{Cpu, Mode};
use crate::xlate::{Access, Regs, TlbOp, Translate};

/// SCTLR.XP: the AP[2:0] permission encoding (ARMv6 extended pages).
pub const SCTLR_XP: u32 = 1 << 23;

/// Fault status: translation fault, section (FS 0b00101).
pub const FS_TRANSLATION_SECTION: u16 = 0x05;
/// Fault status: translation fault, page (FS 0b00111).
pub const FS_TRANSLATION_PAGE: u16 = 0x07;
/// Fault status: domain fault, section (FS 0b01001).
pub const FS_DOMAIN_SECTION: u16 = 0x09;
/// Fault status: domain fault, page (FS 0b01011).
pub const FS_DOMAIN_PAGE: u16 = 0x0B;
/// Fault status: synchronous external abort on a first-level table walk (FS 0b01100).
pub const FS_WALK_FIRST_LEVEL: u16 = 0x0C;
/// Fault status: permission fault, section (FS 0b01101).
pub const FS_PERMISSION_SECTION: u16 = 0x0D;
/// Fault status: synchronous external abort on a second-level table walk (FS 0b01110).
pub const FS_WALK_SECOND_LEVEL: u16 = 0x0E;
/// Fault status: permission fault, page (FS 0b01111).
pub const FS_PERMISSION_PAGE: u16 = 0x0F;

/// Translation-lookaside buffer capacity (entries).
pub const TLB_ENTRIES: usize = 64;

/// One translated region, as its descriptor describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Virtual base of the region, aligned to its size.
    pub va_base: u32,
    /// log2 of the region size: 12 (small page), 16 (large page), 20 (section), 24
    /// (supersection).
    pub size_log2: u8,
    /// Physical base of the region, aligned to its size (up to 40 bits for supersections).
    pub pa_base: u64,
    /// The domain (0 for a supersection).
    pub domain: u8,
    /// True when the region came from a second-level (page) descriptor.
    pub page: bool,
    /// The AP[1:0] field of each subpage (all four equal for sections, large and small pages
    /// with XP = 1 only use index 0).
    pub ap_lo: [u8; 4],
    /// APX (AP[2]).
    pub apx: bool,
    /// log2 of the subpage size that `ap_lo` is indexed by: 10 (4 KiB subpages) or 14.
    pub sub_shift: u8,
    /// Execute-never.
    pub xn: bool,
    /// Privileged execute-never (section bit 0, or the coarse table's bit 2 for its pages).
    pub pxn: bool,
    /// TEX[2:0], recorded only.
    pub tex: u8,
    /// C, recorded only.
    pub c: bool,
    /// B, recorded only.
    pub b: bool,
    /// S (shareable), recorded only.
    pub s: bool,
    /// NS, recorded only.
    pub ns: bool,
    /// nG: the entry is process-specific (tagged with the ASID).
    pub ng: bool,
}

impl Mapping {
    /// The low bits of a virtual address that stay in the region.
    pub fn offset_mask(&self) -> u32 {
        (1u32 << self.size_log2) - 1
    }

    /// The physical address of `va`, which must be inside this region.
    pub fn pa(&self, va: u32) -> u64 {
        self.pa_base | u64::from(va & self.offset_mask())
    }

    /// Whether this region covers `va`.
    pub fn covers(&self, va: u32) -> bool {
        va & !self.offset_mask() == self.va_base
    }
}

/// Walks the translation tables for `va` under `regs`, reading descriptors from `bus`.
/// Returns the region the descriptor describes, or the fault status (DFSR layout, FS in the
/// low nibble, domain in [7:4]). No permission is checked here: see [`check`].
pub fn walk(bus: &mut dyn Bus, regs: &Regs, va: u32) -> Result<Mapping, u16> {
    let n = regs.ttbcr & 7;
    // TTBR1 when N is not 0 and the top N bits of the address are not all zero.
    let (table, mask) = if n != 0 && (va >> (32 - n)) != 0 {
        (regs.ttbr1, 0xFFFF_C000)
    } else {
        // TTBR0[31:14-N]: the table is 16 KiB >> N bytes, aligned to its size.
        (regs.ttbr0, !((0x4000u32 >> n) - 1))
    };
    let l1 = u64::from((table & mask) | ((va >> 20) << 2));
    let d1 = bus.read32(l1).map_err(|_| FS_WALK_FIRST_LEVEL)?;
    match d1 & 3 {
        0 => Err(FS_TRANSLATION_SECTION),
        1 => {
            // Coarse table: bits [31:10] base, [8:5] domain, [3] NS, [2] PXN.
            let domain = ((d1 >> 5) & 0xF) as u8;
            let ns = d1 & (1 << 3) != 0;
            let pxn = d1 & (1 << 2) != 0;
            let l2 = u64::from((d1 & 0xFFFF_FC00) | (((va >> 12) & 0xFF) << 2));
            let d2 = bus.read32(l2).map_err(|_| FS_WALK_SECOND_LEVEL)?;
            page_descriptor(d2, va, domain, ns, pxn)
        }
        _ => Ok(section_descriptor(d1, va)),
    }
}

/// A second-level descriptor: a large page (bits [1:0] = 01) or a small page (1x).
fn page_descriptor(d2: u32, va: u32, domain: u8, ns: bool, pxn: bool) -> Result<Mapping, u16> {
    // AP[1:0] per subpage at [5:4], [7:6], [9:8], [11:10]; APX at [9] (with XP = 1 only [5:4]
    // and bit 9 are used).
    let ap_lo = [
        ((d2 >> 4) & 3) as u8,
        ((d2 >> 6) & 3) as u8,
        ((d2 >> 8) & 3) as u8,
        ((d2 >> 10) & 3) as u8,
    ];
    let common = |size_log2: u8, pa_base: u64, xn: bool, tex: u8, sub_shift: u8| Mapping {
        va_base: va & !((1u32 << size_log2) - 1),
        size_log2,
        pa_base,
        domain,
        page: true,
        ap_lo,
        apx: d2 & (1 << 9) != 0,
        sub_shift,
        xn,
        pxn,
        tex,
        c: d2 & (1 << 3) != 0,
        b: d2 & (1 << 2) != 0,
        s: d2 & (1 << 10) != 0,
        ns,
        ng: d2 & (1 << 11) != 0,
    };
    match d2 & 3 {
        0 => Err(FS_TRANSLATION_PAGE),
        1 => Ok(common(
            16,
            u64::from(d2 & 0xFFFF_0000),
            d2 & (1 << 15) != 0,
            ((d2 >> 12) & 7) as u8,
            14,
        )),
        _ => Ok(common(
            12,
            u64::from(d2 & 0xFFFF_F000),
            d2 & 1 != 0,
            ((d2 >> 6) & 7) as u8,
            10,
        )),
    }
}

/// A first-level section or supersection descriptor (bits [1:0] = 1x).
fn section_descriptor(d1: u32, va: u32) -> Mapping {
    let ap = ((d1 >> 10) & 3) as u8;
    let common = |size_log2: u8, va_base: u32, pa_base: u64, domain: u8| Mapping {
        va_base,
        size_log2,
        pa_base,
        domain,
        page: false,
        ap_lo: [ap; 4],
        apx: d1 & (1 << 15) != 0,
        sub_shift: 10,
        xn: d1 & (1 << 4) != 0,
        pxn: d1 & 1 != 0,
        tex: ((d1 >> 12) & 7) as u8,
        c: d1 & (1 << 3) != 0,
        b: d1 & (1 << 2) != 0,
        s: d1 & (1 << 16) != 0,
        ns: d1 & (1 << 19) != 0,
        ng: d1 & (1 << 17) != 0,
    };
    if d1 & (1 << 18) != 0 {
        // Supersection: PA[31:24] at [31:24], PA[35:32] at [23:20], PA[39:36] at [8:5]; the
        // domain field holds those bits, so the domain is 0.
        let pa = u64::from(d1 & 0xFF00_0000)
            | (u64::from((d1 >> 20) & 0xF) << 32)
            | (u64::from((d1 >> 5) & 0xF) << 36);
        common(24, va & 0xFF00_0000, pa, 0)
    } else {
        common(
            20,
            va & 0xFFF0_0000,
            u64::from(d1 & 0xFFF0_0000),
            ((d1 >> 5) & 0xF) as u8,
        )
    }
}

/// The AP[2:0] table with XP = 1 (ARM ARM Table B3-6, AP[2] is APX).
fn ap_v7_allows(ap: u8, privileged: bool, write: bool) -> bool {
    match ap {
        0b001 => privileged,
        0b010 => privileged || !write,
        0b011 => true,
        0b101 => privileged && !write,
        0b110 | 0b111 => !write,
        // 0b000: no access. 0b100 is UNPREDICTABLE: treated as no access.
        _ => false,
    }
}

/// The subpage AP[1:0] table of XP = 0 with S = R = 0 (the ARMv5 encoding).
fn ap_v5_allows(ap: u8, privileged: bool, write: bool) -> bool {
    match ap {
        0 => false,
        1 => privileged,
        2 => privileged || !write,
        _ => true,
    }
}

/// Checks the domain and the permissions of `m` for an access to `va` under `regs`. A failure
/// is the fault status: a domain fault or a permission fault, section or page, with the domain
/// in DFSR bits [7:4].
pub fn check(m: &Mapping, va: u32, regs: &Regs, access: Access) -> Result<(), u16> {
    let domain_bits = (regs.dacr >> (2 * u32::from(m.domain))) & 3;
    let dom_bits = u16::from(m.domain) << 4;
    let (domain_fs, perm_fs) = if m.page {
        (FS_DOMAIN_PAGE, FS_PERMISSION_PAGE)
    } else {
        (FS_DOMAIN_SECTION, FS_PERMISSION_SECTION)
    };
    match domain_bits {
        0 | 2 => return Err(domain_fs | dom_bits),
        3 => return Ok(()),
        _ => {}
    }
    let privileged = regs.privileged;
    let write = access == Access::Write;
    let allowed = if regs.sctlr & SCTLR_XP != 0 {
        ap_v7_allows((u8::from(m.apx) << 2) | m.ap_lo[0], privileged, write)
    } else {
        let sub = (va >> m.sub_shift) & 3;
        ap_v5_allows(m.ap_lo[sub as usize], privileged, write)
    };
    let executable = access != Access::Fetch || (!m.xn && !(privileged && m.pxn));
    if allowed && executable {
        Ok(())
    } else {
        Err(perm_fs | dom_bits)
    }
}

/// The result of a complete translation, for tests and the ATS operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    /// The physical address.
    pub pa: u64,
    /// The region it came from.
    pub mapping: Mapping,
}

/// A translation without the TLB: walk, then check. This is the MMU's semantics; the TLB only
/// caches the walk.
pub fn translate_uncached(
    bus: &mut dyn Bus,
    regs: &Regs,
    va: u32,
    access: Access,
) -> Result<Translation, u16> {
    let mapping = walk(bus, regs, va)?;
    check(&mapping, va, regs, access)?;
    Ok(Translation {
        pa: mapping.pa(va),
        mapping,
    })
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    m: Mapping,
    asid: u8,
}

impl Entry {
    /// Whether this entry answers for `va` under `asid` (global entries answer any ASID).
    fn matches(&self, va: u32, asid: u8) -> bool {
        self.m.covers(va) && (!self.m.ng || self.asid == asid)
    }
}

/// The MMU as the core's translation hook: the identity mapping while SCTLR.M is clear, the
/// table walk and a [`TLB_ENTRIES`]-entry TLB while it is set.
#[derive(Clone, Debug, Default)]
pub struct Mmu {
    entries: Vec<Entry>,
    next_victim: usize,
}

impl Mmu {
    /// An MMU with an empty TLB.
    pub fn new() -> Mmu {
        Mmu {
            entries: Vec::with_capacity(TLB_ENTRIES),
            next_victim: 0,
        }
    }

    /// The number of valid TLB entries (for tests and debugging).
    pub fn tlb_len(&self) -> usize {
        self.entries.len()
    }

    fn lookup(&self, va: u32, asid: u8) -> Option<Mapping> {
        self.entries
            .iter()
            .find(|e| e.matches(va, asid))
            .map(|e| e.m)
    }

    fn insert(&mut self, m: Mapping, asid: u8) {
        let e = Entry { m, asid };
        if self.entries.len() < TLB_ENTRIES {
            self.entries.push(e);
        } else {
            self.entries[self.next_victim] = e;
            self.next_victim = (self.next_victim + 1) % TLB_ENTRIES;
        }
    }
}

impl Translate for Mmu {
    fn translate(
        &mut self,
        bus: &mut dyn Bus,
        regs: &Regs,
        va: u32,
        access: Access,
    ) -> Result<u64, u16> {
        if regs.sctlr & SCTLR_M == 0 {
            return Ok(u64::from(va));
        }
        let m = match self.lookup(va, regs.asid) {
            Some(m) => m,
            None => {
                let m = walk(bus, regs, va)?;
                self.insert(m, regs.asid);
                m
            }
        };
        check(&m, va, regs, access)?;
        Ok(m.pa(va))
    }

    fn invalidate(&mut self, op: TlbOp) {
        match op {
            TlbOp::All => self.entries.clear(),
            TlbOp::Mva { va, asid } => self.entries.retain(|e| !e.matches(va, asid)),
            TlbOp::MvaAll { va } => self.entries.retain(|e| !e.m.covers(va)),
            TlbOp::Asid { asid } => self.entries.retain(|e| !(e.m.ng && e.asid == asid)),
        }
        self.next_victim = 0;
    }
}

impl Cpu {
    /// The translation registers for an access of the given privilege. An LDRT/STRT in a
    /// privileged mode is unprivileged: the caller sets `unpriv` around it.
    pub(crate) fn xlate_regs(&self, privileged: bool) -> Regs {
        Regs {
            sctlr: self.cp15.sctlr,
            ttbr0: self.cp15.ttbr0,
            ttbr1: self.cp15.ttbr1,
            ttbcr: self.cp15.ttbcr,
            dacr: self.cp15.dacr,
            asid: (self.cp15.contextidr & 0xFF) as u8,
            privileged: privileged && !self.unpriv,
        }
    }

    /// Registers for a data access: privileged unless the core is in User mode.
    pub(crate) fn data_regs(&self) -> Regs {
        self.xlate_regs(self.mode() != Mode::User)
    }

    /// A TLB maintenance operation from `MCR p15, 0, Rt, c8, CRm, opc2` (`v` is Rt).
    pub(crate) fn tlb_op(&mut self, crm: u32, opc2: u32, v: u32) {
        let op = match (crm, opc2) {
            (7, 0) | (3, 0) => TlbOp::All,
            (7, 1) | (3, 1) => TlbOp::Mva {
                va: v & !0xFFF,
                asid: v as u8,
            },
            (7, 2) | (3, 2) => TlbOp::Asid { asid: v as u8 },
            (7, 3) | (3, 3) => TlbOp::MvaAll { va: v & !0xFFF },
            _ => return,
        };
        self.xlate.invalidate(op);
    }

    /// An address translation operation from `MCR p15, 0, Rt, c7, c8, opc2`: ATS1CPR (0),
    /// ATS1CPW (1), ATS1CUR (2), ATS1CUW (3). The result goes to PAR. It does not fill the TLB.
    pub(crate) fn ats(&mut self, bus: &mut dyn Bus, opc2: u32, va: u32) {
        let (access, privileged) = match opc2 {
            0 => (Access::Read, true),
            1 => (Access::Write, true),
            2 => (Access::Read, false),
            3 => (Access::Write, false),
            _ => return,
        };
        let regs = self.xlate_regs(privileged);
        self.cp15.par = if regs.sctlr & SCTLR_M == 0 {
            // With the MMU off the operation returns the flat mapping.
            va & 0xFFFF_F000
        } else {
            match translate_uncached(bus, &regs, va, access) {
                // PAR success: PA[31:12], NS at bit 9, F clear.
                Ok(t) => (t.pa as u32 & 0xFFFF_F000) | if t.mapping.ns { 1 << 9 } else { 0 },
                // PAR fault: F set, the fault status in FST (simplified: domain not encoded).
                Err(fs) => 1 | (u32::from(fs & 0xF) << 1),
            }
        };
    }
}
