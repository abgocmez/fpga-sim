//! The address-translation hook. The core's default is [`crate::mmu::Mmu`], the ARMv7-A
//! short-descriptor MMU: with SCTLR.M clear it is the identity mapping, with M set it walks the
//! translation tables in the bus. [`Identity`] is a hook with no translation at all, for callers
//! that want flat physical addressing whatever SCTLR says.

use ps_bus::Bus;

/// What an access is for, as the translation table walk and permission checks need it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// An instruction fetch.
    Fetch,
    /// A data read.
    Read,
    /// A data write.
    Write,
}

/// The translation-relevant state of the core for one access: a copy of the CP15 registers
/// and the privilege of the access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Regs {
    /// SCTLR (M, A, XP, ... are read by the MMU).
    pub sctlr: u32,
    /// TTBR0.
    pub ttbr0: u32,
    /// TTBR1.
    pub ttbr1: u32,
    /// TTBCR (the N field selects TTBR0 or TTBR1).
    pub ttbcr: u32,
    /// DACR (two bits per domain).
    pub dacr: u32,
    /// CONTEXTIDR[7:0]: the ASID that tags the non-global TLB entries.
    pub asid: u8,
    /// True for a privileged access: not from User mode, and not an LDRT/STRT.
    pub privileged: bool,
}

/// A TLB maintenance operation (CP15 c8). The operand is already decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlbOp {
    /// TLBIALL: every entry.
    All,
    /// TLBIMVA: the entries covering `va` that are global or tagged with `asid`.
    Mva {
        /// The virtual address (its page offset is ignored).
        va: u32,
        /// The ASID operand (`Rt[7:0]`).
        asid: u8,
    },
    /// TLBIMVAA: the entries covering `va`, whatever their ASID.
    MvaAll {
        /// The virtual address.
        va: u32,
    },
    /// TLBIASID: the non-global entries tagged with `asid`.
    Asid {
        /// The ASID operand.
        asid: u8,
    },
}

/// Maps a virtual address to a physical one. A failure is the fault status in DFSR layout:
/// FS in bits [3:0], the domain in [7:4] (see [`crate::mmu`]). The core adds WnR itself.
pub trait Translate {
    /// Translates `va` for an `access` under `regs`. `bus` is where a table walk reads its
    /// descriptors from.
    fn translate(
        &mut self,
        bus: &mut dyn Bus,
        regs: &Regs,
        va: u32,
        access: Access,
    ) -> Result<u64, u16>;

    /// Applies a TLB maintenance operation. A hook without a TLB ignores it.
    fn invalidate(&mut self, _op: TlbOp) {}
}

/// A hook with no translation: physical = virtual, no faults, whatever SCTLR.M says.
#[derive(Clone, Copy, Debug, Default)]
pub struct Identity;

impl Translate for Identity {
    fn translate(
        &mut self,
        _bus: &mut dyn Bus,
        _regs: &Regs,
        va: u32,
        _access: Access,
    ) -> Result<u64, u16> {
        Ok(u64::from(va))
    }
}
