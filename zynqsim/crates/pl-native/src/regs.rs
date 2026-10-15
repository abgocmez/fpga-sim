use pl_api::{BusError, BusResult, PlTarget, PsPort};

/// A block of 32-bit memory-mapped registers.
pub trait RegisterBlock: Send {
    /// Name for diagnostics.
    fn name(&self) -> &str;
    /// Size of the address window in bytes.
    fn size(&self) -> u64;
    /// Reads the word at a 4-byte aligned `offset`.
    fn read32(&mut self, offset: u64) -> u32;
    /// Writes the word at a 4-byte aligned `offset`; bit *n* of `strobe`
    /// enables byte lane *n*.
    fn write32(&mut self, offset: u64, value: u32, strobe: u8);
    /// Returns the block to its power-on state.
    fn reset(&mut self);
}

/// A register block placed at a base address on one GP port.
pub struct Mapped {
    /// `M_AXI_GP` port index (0 or 1).
    pub gp: u8,
    /// Base physical address.
    pub base: u64,
    /// The block.
    pub block: Box<dyn RegisterBlock>,
}

/// A PL made of native register blocks.
///
/// Wide or unaligned accesses are split into 32-bit beats with byte strobes,
/// the way an AXI4-Lite bridge presents them. While `FCLK_RESET0_N` is low
/// (wire 0 of the PS-to-PL wires) blocks are held in reset.
#[derive(Default)]
pub struct NativePl {
    blocks: Vec<Mapped>,
    in_reset: bool,
    /// Number of bus accesses served, for statistics.
    pub accesses: u64,
}

impl NativePl {
    /// An empty PL.
    pub fn new() -> NativePl {
        NativePl::default()
    }

    /// Maps `block` at `base` on `M_AXI_GP<gp>`. Panics if the window overlaps
    /// an existing one (a configuration bug, not a runtime condition).
    pub fn map(&mut self, gp: u8, base: u64, block: Box<dyn RegisterBlock>) {
        let end = base + block.size();
        for m in &self.blocks {
            let mend = m.base + m.block.size();
            assert!(
                m.gp != gp || end <= m.base || base >= mend,
                "{} overlaps {} on GP{gp}",
                block.name(),
                m.block.name()
            );
        }
        self.blocks.push(Mapped { gp, base, block });
    }

    fn find(&mut self, gp: u8, addr: u64) -> Option<(&mut Mapped, u64)> {
        self.blocks.iter_mut().find_map(|m| {
            let off = addr.checked_sub(m.base)?;
            (m.gp == gp && off < m.block.size()).then_some((m, off))
        })
    }
}

impl PlTarget for NativePl {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        _now_ns: u64,
        _ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.accesses += 1;
        let mut done = 0usize;
        while done < buf.len() {
            let a = addr + done as u64;
            let lane = (a & 3) as usize;
            let n = (4 - lane).min(buf.len() - done);
            let (m, off) = self.find(gp, a & !3).ok_or(BusError::Decode)?;
            let word = m.block.read32(off & !3).to_le_bytes();
            buf[done..done + n].copy_from_slice(&word[lane..lane + n]);
            done += n;
        }
        Ok(())
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        _now_ns: u64,
        _ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.accesses += 1;
        let mut done = 0usize;
        while done < data.len() {
            let a = addr + done as u64;
            let lane = (a & 3) as usize;
            let n = (4 - lane).min(data.len() - done);
            let mut word = [0u8; 4];
            word[lane..lane + n].copy_from_slice(&data[done..done + n]);
            let strobe = (((1u16 << n) - 1) << lane) as u8;
            let in_reset = self.in_reset;
            let (m, off) = self.find(gp, a & !3).ok_or(BusError::Decode)?;
            if !in_reset {
                m.block.write32(off & !3, u32::from_le_bytes(word), strobe);
            }
            done += n;
        }
        Ok(())
    }

    fn wire(&mut self, line: u32, level: bool, _now_ns: u64) {
        // Line 0 of the PS-to-PL wires is FCLK_RESET0_N (active low).
        if line == 0 {
            let reset = !level;
            if reset && !self.in_reset {
                for m in &mut self.blocks {
                    m.block.reset();
                }
            }
            self.in_reset = reset;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MathIp;
    use pl_api::PsMasterPort;

    struct NoPs;
    impl PsPort for NoPs {
        fn mem_read(&mut self, _: PsMasterPort, _: u64, _: &mut [u8]) -> BusResult<()> {
            Err(BusError::Decode)
        }
        fn mem_write(&mut self, _: PsMasterPort, _: u64, _: &[u8]) -> BusResult<()> {
            Err(BusError::Decode)
        }
        fn irq(&mut self, _: u32, _: bool, _: u64) -> BusResult<()> {
            Ok(())
        }
    }

    fn pl() -> NativePl {
        let mut p = NativePl::new();
        p.map(0, 0x43C0_0000, Box::new(MathIp::new()));
        p
    }

    #[test]
    fn word_access_and_decode_errors() {
        let mut p = pl();
        let mut ps = NoPs;
        p.write(0, 0x43C0_0000, &21u32.to_le_bytes(), 0, &mut ps)
            .unwrap();
        let mut b = [0u8; 4];
        p.read(0, 0x43C0_0004, &mut b, 0, &mut ps).unwrap();
        assert_eq!(u32::from_le_bytes(b), 42);
        // Wrong port, outside the window, below the window.
        assert_eq!(
            p.read(1, 0x43C0_0000, &mut b, 0, &mut ps),
            Err(BusError::Decode)
        );
        assert_eq!(
            p.read(0, 0x43C0_1000, &mut b, 0, &mut ps),
            Err(BusError::Decode)
        );
        assert_eq!(
            p.read(0, 0x43BF_FFFC, &mut b, 0, &mut ps),
            Err(BusError::Decode)
        );
    }

    #[test]
    fn splits_wide_and_unaligned_accesses() {
        let mut p = pl();
        let mut ps = NoPs;
        // 8-byte write covers reg0 and (read-only) reg1.
        p.write(0, 0x43C0_0000, &[1, 2, 3, 4, 9, 9, 9, 9], 0, &mut ps)
            .unwrap();
        let mut w = [0u8; 8];
        p.read(0, 0x43C0_0000, &mut w, 0, &mut ps).unwrap();
        assert_eq!(&w[..4], &[1, 2, 3, 4]);
        assert_eq!(
            u32::from_le_bytes([w[4], w[5], w[6], w[7]]),
            0x0403_0201u32 * 2
        );
        // A single-byte write to lane 2 of reg0.
        p.write(0, 0x43C0_0002, &[0xAA], 0, &mut ps).unwrap();
        let mut b = [0u8; 4];
        p.read(0, 0x43C0_0000, &mut b, 0, &mut ps).unwrap();
        assert_eq!(b, [1, 2, 0xAA, 4]);
        // Single-byte read from lane 3.
        let mut one = [0u8; 1];
        p.read(0, 0x43C0_0003, &mut one, 0, &mut ps).unwrap();
        assert_eq!(one, [4]);
    }

    #[test]
    fn reset_wire_clears_state_and_blocks_writes() {
        let mut p = pl();
        let mut ps = NoPs;
        p.write(0, 0x43C0_0000, &5u32.to_le_bytes(), 0, &mut ps)
            .unwrap();
        p.wire(0, false, 10); // FCLK_RESET0_N asserted
        let mut b = [0u8; 4];
        p.read(0, 0x43C0_0000, &mut b, 11, &mut ps).unwrap();
        assert_eq!(u32::from_le_bytes(b), 0);
        p.write(0, 0x43C0_0000, &7u32.to_le_bytes(), 12, &mut ps)
            .unwrap();
        p.read(0, 0x43C0_0000, &mut b, 13, &mut ps).unwrap();
        assert_eq!(u32::from_le_bytes(b), 0, "writes are ignored in reset");
        p.wire(0, true, 20);
        p.write(0, 0x43C0_0000, &7u32.to_le_bytes(), 21, &mut ps)
            .unwrap();
        p.read(0, 0x43C0_0004, &mut b, 22, &mut ps).unwrap();
        assert_eq!(u32::from_le_bytes(b), 14);
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn overlapping_windows_are_a_configuration_error() {
        let mut p = pl();
        p.map(0, 0x43C0_0800, Box::new(MathIp::new()));
    }
}
