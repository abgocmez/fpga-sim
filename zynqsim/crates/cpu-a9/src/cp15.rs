//! System control coprocessor (CP15) registers of one Cortex-A9 core.
//!
//! Only the registers the bare-metal firmware and the Xilinx BSP touch are modelled. Cache and
//! TLB maintenance operations are accepted as no-ops (caches are not modelled: the bus is
//! coherent). Any other encoding is reported as unknown so the caller sees it.

/// SCTLR.M: MMU enable. The core refuses to run with it set (no MMU yet).
pub(crate) const SCTLR_M: u32 = 1 << 0;
/// SCTLR.A: strict alignment checking.
pub(crate) const SCTLR_A: u32 = 1 << 1;
/// SCTLR.TE: exceptions are taken in Thumb state.
pub(crate) const SCTLR_TE: u32 = 1 << 30;
/// SCTLR.V: high exception vectors (0xFFFF0000).
pub(crate) const SCTLR_V: u32 = 1 << 13;

/// SCTLR reset value on the Cortex-A9 (MMU, caches and alignment off).
const SCTLR_RESET: u32 = 0x00C5_0078;
/// MIDR of a Cortex-A9 r3p0 (the Zynq-7000 part).
const MIDR: u32 = 0x413F_C090;
/// CTR: 32 KiB, 32-byte-line L1 caches, the Cortex-A9 value.
const CTR: u32 = 0x8444_C004;

/// The CP15 state of one core.
#[derive(Clone, Debug)]
pub(crate) struct Cp15 {
    /// Core number in MPIDR[1:0].
    pub(crate) cpu_id: u32,
    pub(crate) sctlr: u32,
    pub(crate) actlr: u32,
    pub(crate) cpacr: u32,
    pub(crate) ttbr0: u32,
    pub(crate) ttbr1: u32,
    pub(crate) ttbcr: u32,
    pub(crate) dacr: u32,
    pub(crate) dfsr: u32,
    pub(crate) ifsr: u32,
    pub(crate) dfar: u32,
    pub(crate) ifar: u32,
    pub(crate) vbar: u32,
    pub(crate) csselr: u32,
    pub(crate) contextidr: u32,
    pub(crate) tpidr: [u32; 3],
}

impl Cp15 {
    /// The reset state for core `cpu_id`.
    pub(crate) fn new(cpu_id: u32) -> Cp15 {
        Cp15 {
            cpu_id: cpu_id & 3,
            sctlr: SCTLR_RESET,
            actlr: 0,
            cpacr: 0,
            ttbr0: 0,
            ttbr1: 0,
            ttbcr: 0,
            dacr: 0,
            dfsr: 0,
            ifsr: 0,
            dfar: 0,
            ifar: 0,
            vbar: 0,
            csselr: 0,
            contextidr: 0,
            tpidr: [0; 3],
        }
    }

    /// Reads `(opc1, CRn, CRm, opc2)`; `None` if the register is not modelled.
    pub(crate) fn read(&self, opc1: u32, crn: u32, crm: u32, opc2: u32) -> Option<u32> {
        let v = match (opc1, crn, crm, opc2) {
            (0, 0, 0, 0) => MIDR,
            (0, 0, 0, 1) => CTR,
            (0, 0, 0, 2) => 0, // TCMTR: no TCM
            (0, 0, 0, 5) => 0x8000_0000 | self.cpu_id,
            (2, 0, 0, 0) => self.csselr,
            (0, 1, 0, 0) => self.sctlr,
            (0, 1, 0, 1) => self.actlr,
            (0, 1, 0, 2) => self.cpacr,
            (0, 2, 0, 0) => self.ttbr0,
            (0, 2, 0, 1) => self.ttbr1,
            (0, 2, 0, 2) => self.ttbcr,
            (0, 3, 0, 0) => self.dacr,
            (0, 5, 0, 0) => self.dfsr,
            (0, 5, 0, 1) => self.ifsr,
            (0, 6, 0, 0) => self.dfar,
            (0, 6, 0, 2) => self.ifar,
            (0, 12, 0, 0) => self.vbar,
            (0, 13, 0, 1) => self.contextidr,
            (0, 13, 0, 2) => self.tpidr[0],
            (0, 13, 0, 3) => self.tpidr[1],
            (0, 13, 0, 4) => self.tpidr[2],
            _ => return None,
        };
        Some(v)
    }

    /// Writes `(opc1, CRn, CRm, opc2)`; `false` if the register is not modelled.
    pub(crate) fn write(&mut self, opc1: u32, crn: u32, crm: u32, opc2: u32, v: u32) -> bool {
        match (opc1, crn, crm, opc2) {
            // Cache and TLB maintenance: accepted, no state.
            (0, 7, _, _) | (0, 8, _, _) => {}
            (2, 0, 0, 0) => self.csselr = v,
            (0, 1, 0, 0) => self.sctlr = v,
            (0, 1, 0, 1) => self.actlr = v,
            (0, 1, 0, 2) => self.cpacr = v,
            (0, 2, 0, 0) => self.ttbr0 = v,
            (0, 2, 0, 1) => self.ttbr1 = v,
            (0, 2, 0, 2) => self.ttbcr = v,
            (0, 3, 0, 0) => self.dacr = v,
            (0, 5, 0, 0) => self.dfsr = v,
            (0, 5, 0, 1) => self.ifsr = v,
            (0, 6, 0, 0) => self.dfar = v,
            (0, 6, 0, 2) => self.ifar = v,
            (0, 12, 0, 0) => self.vbar = v & !0x1F,
            (0, 13, 0, 1) => self.contextidr = v,
            (0, 13, 0, 2) => self.tpidr[0] = v,
            (0, 13, 0, 3) => self.tpidr[1] = v,
            (0, 13, 0, 4) => self.tpidr[2] = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Cp15;

    #[test]
    fn mpidr_carries_the_core_number_and_actlr_keeps_the_smp_bit() {
        for id in 0..2u32 {
            let cp = Cp15::new(id);
            assert_eq!(
                cp.read(0, 0, 0, 5),
                Some(0x8000_0000 | id),
                "MPIDR of core {id}"
            );
        }
        let mut cp = Cp15::new(1);
        // ACTLR bit 6 is SMP: the core joins the coherent cluster when it is set.
        assert!(cp.write(0, 1, 0, 1, 1 << 6));
        assert_eq!(cp.read(0, 1, 0, 1), Some(1 << 6));
    }
}
