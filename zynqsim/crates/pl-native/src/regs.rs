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

    /// Brings the block's notion of time up to `now_ns` (PS time, monotone).
    /// Called before every access and on every lock-step `advance`; models
    /// with timers, baud-paced FIFOs and similar use it. The default does
    /// nothing, which is what a purely combinational register block wants.
    fn advance(&mut self, _now_ns: u64) {}

    /// Level of the block's interrupt output. Polled after `advance`; a
    /// change is forwarded to the PS on the block's mapped `IRQ_F2P` line.
    fn irq_level(&self) -> bool {
        false
    }

    /// Level of interrupt output `output`: 0 is [`RegisterBlock::irq_level`]; a block with more
    /// outputs (the AXI DMA has two) overrides this.
    fn irq_output(&self, output: usize) -> bool {
        output == 0 && self.irq_level()
    }

    /// Lets the block act as a bus master (a DMA engine): called after every bus access and
    /// on every lock-step `advance`, with the PS port, so the block can read and write PS
    /// memory and finish a transfer whose time has come. The default does nothing.
    fn service(&mut self, _now_ns: u64, _ps: &mut dyn PsPort) {}

    /// Another block's interrupt output, routed to this block with
    /// [`NativePl::route_irq`], changed to `level` (input number `input`). An interrupt
    /// controller implements it. The default ignores the input.
    fn set_irq_input(&mut self, _input: u32, _level: bool, _now_ns: u64) {}
}

/// A register block placed at a base address on one GP port.
pub struct Mapped {
    /// `M_AXI_GP` port index (0 or 1).
    pub gp: u8,
    /// Base physical address.
    pub base: u64,
    /// The block.
    pub block: Box<dyn RegisterBlock>,
    /// `IRQ_F2P` line (0..=15) the block's interrupt output drives, if any.
    pub irq: Option<u32>,
    /// Level last reported to the PS on `irq`.
    irq_state: bool,
    /// Further interrupt outputs (a DMA's second one) and routes to other blocks, see
    /// [`NativePl::route_irq`].
    outs: Vec<IrqOut>,
}

/// Where a block's interrupt output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrqDest {
    /// An `IRQ_F2P` line (0..=15).
    F2p(u32),
    /// Input `input` of the block named `block` (an interrupt controller).
    Block {
        /// Name of the target block ([`RegisterBlock::name`]).
        block: String,
        /// Its input number.
        input: u32,
    },
}

struct IrqOut {
    output: usize,
    dest: IrqDest,
    state: bool,
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
    /// Some block's interrupt output is routed to another block.
    has_routes: bool,
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
        self.blocks.push(Mapped {
            gp,
            base,
            block,
            irq: None,
            irq_state: false,
            outs: Vec::new(),
        });
    }

    /// Sends interrupt output `output` (0 is [`RegisterBlock::irq_level`]) of the already mapped
    /// block named `block` to `dest`: an `IRQ_F2P` line, or an input of another block (an
    /// interrupt controller, through [`RegisterBlock::set_irq_input`]; the target may be mapped
    /// before or after). Returns false when there is no block of that name.
    pub fn route_irq(&mut self, block: &str, output: usize, dest: IrqDest) -> bool {
        let Some(m) = self.blocks.iter_mut().find(|m| m.block.name() == block) else {
            return false;
        };
        if matches!(dest, IrqDest::Block { .. }) {
            self.has_routes = true;
        }
        m.outs.push(IrqOut {
            output,
            dest,
            state: false,
        });
        true
    }

    /// Delivers every changed block-to-block interrupt level. Two passes, so a chain of two
    /// controllers settles.
    fn route_irqs(&mut self, now_ns: u64) {
        if !self.has_routes {
            return;
        }
        for _ in 0..2 {
            let mut changes: Vec<(usize, usize, usize, u32, bool)> = Vec::new();
            for (i, m) in self.blocks.iter().enumerate() {
                for (k, o) in m.outs.iter().enumerate() {
                    let IrqDest::Block { block, input } = &o.dest else {
                        continue;
                    };
                    let level = m.block.irq_output(o.output);
                    if level != o.state {
                        if let Some(t) = self.blocks.iter().position(|b| b.block.name() == block) {
                            changes.push((i, k, t, *input, level));
                        }
                    }
                }
            }
            if changes.is_empty() {
                break;
            }
            for (i, k, t, input, level) in changes {
                self.blocks[i].outs[k].state = level;
                self.blocks[t].block.set_irq_input(input, level, now_ns);
            }
        }
    }

    /// Brings every block up to `now_ns` and delivers routed interrupts, so a controller read
    /// sees what its sources did before the access.
    fn refresh_routes(&mut self, now_ns: u64) {
        if self.has_routes {
            for m in &mut self.blocks {
                m.block.advance(now_ns);
            }
            self.route_irqs(now_ns);
        }
    }

    /// Like [`NativePl::map`], and drives `IRQ_F2P[irq]` from the block's
    /// [`RegisterBlock::irq_level`]. A change caused by a bus access is sent
    /// with that access; one caused by time passing (a timer) or by a board
    /// input reaches the PS on the next lock-step [`PlTarget::advance`], at
    /// most one quantum later.
    pub fn map_irq(&mut self, gp: u8, base: u64, block: Box<dyn RegisterBlock>, irq: Option<u32>) {
        self.map(gp, base, block);
        if let Some(m) = self.blocks.last_mut() {
            m.irq = irq;
        }
    }

    /// The `(gp, base, size, name)` of every mapped window.
    pub fn windows(&self) -> Vec<(u8, u64, u64, String)> {
        self.blocks
            .iter()
            .map(|m| (m.gp, m.base, m.block.size(), m.block.name().to_string()))
            .collect()
    }

    /// Forwards a change of any block's interrupt output to the PS.
    ///
    /// Called at the end of every bus access and on every lock-step `advance`. Sending the
    /// change from inside the access that caused it (the way an RTL design's interrupt reaches
    /// the PS while an access is outstanding) is what lets a level interrupt fall before the
    /// guest's handler returns; waiting for the next sync would make it fire again.
    fn sync_irqs(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        // Bus masters (DMA) first: a transfer may finish and raise an interrupt.
        for m in &mut self.blocks {
            m.block.service(now_ns, ps);
        }
        self.route_irqs(now_ns);
        for m in &mut self.blocks {
            if let Some(line) = m.irq {
                let level = m.block.irq_level();
                if level != m.irq_state {
                    m.irq_state = level;
                    // A PS that rejects the update is not the model's problem.
                    let _ = ps.irq(line, level, now_ns);
                }
            }
            for o in &mut m.outs {
                if let IrqDest::F2p(line) = o.dest {
                    let level = m.block.irq_output(o.output);
                    if level != o.state {
                        o.state = level;
                        let _ = ps.irq(line, level, now_ns);
                    }
                }
            }
        }
    }

    /// True when `addr` on `M_AXI_GP<gp>` is inside a mapped window.
    pub fn decodes(&self, gp: u8, addr: u64) -> bool {
        self.blocks.iter().any(|m| {
            m.gp == gp
                && addr
                    .checked_sub(m.base)
                    .is_some_and(|off| off < m.block.size())
        })
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
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.accesses += 1;
        self.refresh_routes(now_ns);
        let mut done = 0usize;
        while done < buf.len() {
            let a = addr + done as u64;
            let lane = (a & 3) as usize;
            let n = (4 - lane).min(buf.len() - done);
            let (m, off) = self.find(gp, a & !3).ok_or(BusError::Decode)?;
            m.block.advance(now_ns);
            let word = m.block.read32(off & !3).to_le_bytes();
            buf[done..done + n].copy_from_slice(&word[lane..lane + n]);
            done += n;
        }
        self.sync_irqs(now_ns, ps);
        Ok(())
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.accesses += 1;
        self.refresh_routes(now_ns);
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
            m.block.advance(now_ns);
            if !in_reset {
                m.block.write32(off & !3, u32::from_le_bytes(word), strobe);
            }
            done += n;
        }
        self.sync_irqs(now_ns, ps);
        Ok(())
    }

    fn advance(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        for m in &mut self.blocks {
            m.block.advance(now_ns);
        }
        self.sync_irqs(now_ns, ps);
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
