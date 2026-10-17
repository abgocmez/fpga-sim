//! The Cortex-A9 MPCore interrupt controller: the GIC distributor and the per-core CPU
//! interface, after the ARM PL390 (GICv1) as described in the Cortex-A9 TRM.
//!
//! Model scope: 96 interrupt IDs (SGI 0..=15, PPI 16..=31, SPI 32..=95), two cores, priority
//! and target registers, enable/pending/active state, level and edge inputs, software
//! generated interrupts, the priority mask and running priority, acknowledge and end of
//! interrupt. Not modelled: security extensions, the binary-point split of priorities, and
//! the interrupt-group registers.
//!
//! Register offsets are relative to the distributor base (0xF8F01000) or the CPU interface
//! base (0xF8F00100). `VERIFY:` the identification registers and the ICDICTR value.

use mem_sys::{IrqInput, lane_merge, lane_read};
use ps_bus::BusResult;

/// Number of cores the MPCore is modelled with.
pub const NCPU: usize = 2;
/// Number of interrupt IDs on Zynq-7000.
pub const NUM_IRQS: usize = 96;
/// The value an acknowledge returns when no interrupt is taken.
pub const SPURIOUS: u32 = 1023;
/// Running priority when no interrupt is active.
const IDLE_PRIO: u16 = 256;
/// Identification value of the distributor and the CPU interface (`VERIFY:`).
const IIDR: u32 = 0x0000_043B;

/// The state of one interrupt input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Line {
    enabled: bool,
    prio: u8,
    /// Edge-triggered (`true`) or level-sensitive.
    edge: bool,
    /// The last level the source reported.
    line: bool,
    /// Pending from the hardware line.
    hw: bool,
    /// Pending from software (ICDISPR, or an SGI for banked ids).
    sw: bool,
}

impl Line {
    /// Applies one level from the source. An edge-triggered line latches a rising edge; a
    /// level-sensitive line is pending while the level is high.
    fn apply(&mut self, level: bool) {
        if self.edge {
            if level && !self.line {
                self.hw = true;
            }
        } else {
            self.hw = level;
        }
        self.line = level;
    }
}

/// The interrupt controller. Banked state (SGIs, PPIs and the CPU interface) has one copy per
/// core; the register accesses of the core selected by [`Gic::set_cpu`] reach its copy.
pub struct Gic {
    dist_enabled: bool,
    cur: usize,
    /// IDs 0..32 per core: SGIs use `enabled`, `prio`; PPIs use all fields.
    own: [[Line; 32]; NCPU],
    /// Pending SGIs per target core, per SGI ID: bit n set = sent by core n.
    sgi: [[u8; 16]; NCPU],
    /// SPIs 32..96.
    shared: [Line; NUM_IRQS - 32],
    /// Target core mask of each SPI.
    targets: [u8; NUM_IRQS - 32],
    cpu_enabled: [bool; NCPU],
    pmr: [u8; NCPU],
    bpr: [u8; NCPU],
    abpr: [u8; NCPU],
    /// Active interrupts per core, innermost last: (id, priority).
    active: [Vec<(u16, u8)>; NCPU],
}

impl Default for Gic {
    fn default() -> Self {
        Gic::new()
    }
}

impl Gic {
    /// A controller in its reset state: everything disabled, priorities zero.
    pub fn new() -> Gic {
        Gic {
            dist_enabled: false,
            cur: 0,
            own: [[Line::default(); 32]; NCPU],
            sgi: [[0; 16]; NCPU],
            shared: [Line::default(); NUM_IRQS - 32],
            targets: [0; NUM_IRQS - 32],
            cpu_enabled: [false; NCPU],
            pmr: [0; NCPU],
            bpr: [0; NCPU],
            abpr: [0; NCPU],
            active: [Vec::new(), Vec::new()],
        }
    }

    /// Selects the core whose banked registers the next access reaches.
    pub fn set_cpu(&mut self, cpu: usize) {
        self.cur = cpu.min(NCPU - 1);
    }

    /// Applies the interrupt levels of the devices: PPIs carry their core, SPIs have none.
    pub fn accept(&mut self, lines: &[IrqInput]) {
        for l in lines {
            let id = usize::from(l.id);
            match (id, l.cpu) {
                (16..=31, Some(c)) if usize::from(c) < NCPU => {
                    self.own[usize::from(c)][id].apply(l.level);
                }
                (32..=95, None) => self.shared[id - 32].apply(l.level),
                _ => {}
            }
        }
    }

    /// Whether the interrupt output to core `cpu` is asserted.
    pub fn signal(&self, cpu: usize) -> bool {
        if !self.cpu_enabled[cpu] {
            return false;
        }
        match self.best(cpu) {
            Some((_, p)) => {
                u16::from(p) < u16::from(self.pmr[cpu]) && u16::from(p) < self.running(cpu)
            }
            None => false,
        }
    }

    /// ICCIAR: takes the highest-priority pending interrupt of `cpu` and makes it active.
    /// Returns [`SPURIOUS`] if none qualifies. An SGI returns its source core in bits 12:10.
    pub fn ack(&mut self, cpu: usize) -> u32 {
        if !self.signal(cpu) {
            return SPURIOUS;
        }
        let Some((id, prio)) = self.best(cpu) else {
            return SPURIOUS;
        };
        let mut src = 0u32;
        if id < 16 {
            let mask = self.sgi[cpu][id];
            src = mask.trailing_zeros();
            self.sgi[cpu][id] &= !(1u8 << src);
        } else {
            let l = self.line_mut(cpu, id);
            if l.edge {
                l.hw = false;
            }
            l.sw = false;
        }
        self.active[cpu].push((id as u16, prio));
        (src << 10) | id as u32
    }

    /// ICCEOIR: ends the innermost active interrupt with ID `value & 0x3FF`.
    pub fn eoi(&mut self, cpu: usize, value: u32) {
        let id = (value & 0x3FF) as u16;
        if let Some(pos) = self.active[cpu].iter().rposition(|&(i, _)| i == id) {
            self.active[cpu].remove(pos);
        }
    }

    /// ICCHPIR: the ID of the highest-priority pending interrupt, or [`SPURIOUS`].
    pub fn highest_pending(&self, cpu: usize) -> u32 {
        self.best(cpu).map_or(SPURIOUS, |(id, _)| id as u32)
    }

    /// The priority of the innermost active interrupt of `cpu`, or the idle value.
    fn running(&self, cpu: usize) -> u16 {
        self.active[cpu]
            .last()
            .map_or(IDLE_PRIO, |&(_, p)| u16::from(p))
    }

    fn line(&self, cpu: usize, id: usize) -> &Line {
        if id < 32 {
            &self.own[cpu][id]
        } else {
            &self.shared[id - 32]
        }
    }

    fn line_mut(&mut self, cpu: usize, id: usize) -> &mut Line {
        if id < 32 {
            &mut self.own[cpu][id]
        } else {
            &mut self.shared[id - 32]
        }
    }

    fn is_active(&self, id: usize) -> bool {
        self.active
            .iter()
            .any(|s| s.iter().any(|&(i, _)| usize::from(i) == id))
    }

    /// Pending as software reads it: any source, whatever the target.
    fn pending_raw(&self, cpu: usize, id: usize) -> bool {
        if id < 16 {
            self.sgi[cpu][id] != 0
        } else {
            let l = self.line(cpu, id);
            l.hw || l.sw
        }
    }

    /// Pending for delivery to `cpu`: an SPI only to the cores in its target mask.
    fn pending_for(&self, cpu: usize, id: usize) -> bool {
        if id >= 32 && self.targets[id - 32] & (1 << cpu) == 0 {
            return false;
        }
        self.pending_raw(cpu, id)
    }

    /// The best deliverable interrupt for `cpu`: lowest priority value, then lowest ID.
    fn best(&self, cpu: usize) -> Option<(usize, u8)> {
        if !self.dist_enabled {
            return None;
        }
        let mut best: Option<(u8, usize)> = None;
        for id in 0..NUM_IRQS {
            let l = self.line(cpu, id);
            if !l.enabled || !self.pending_for(cpu, id) || self.is_active(id) {
                continue;
            }
            if best.is_none_or(|(p, _)| l.prio < p) {
                best = Some((l.prio, id));
            }
        }
        best.map(|(p, id)| (id, p))
    }

    // ---- distributor -------------------------------------------------------------------

    /// Whether a distributor register is read-modify-write (its unwritten lanes keep their
    /// value). The set/clear registers are not: an unwritten lane is zero, which does nothing.
    fn dist_is_rw(a: u64) -> bool {
        matches!(a, 0x000 | 0x400..=0x5FF | 0x800..=0x9FF | 0xC00..=0xC17)
    }

    /// Word value of a distributor register, as a read returns it.
    fn dist_word(&self, a: u64) -> u32 {
        let cur = self.cur;
        match a {
            0x000 => u32::from(self.dist_enabled),
            0x004 => 0x0000_0022, // ITLinesNumber 2 (96 IDs), CPUNumber 1 (two cores). VERIFY
            0x008 => IIDR,
            0x100..=0x10F | 0x180..=0x18F => self.enable_word(cur, word_index(a)),
            0x200..=0x20F | 0x280..=0x28F => self.pending_word(cur, word_index(a)),
            0x300..=0x30F => self.active_word(word_index(a)),
            0x400..=0x5FF => self.prio_word(cur, ((a - 0x400) / 4) as usize),
            0x800..=0x9FF => self.targets_word(cur, ((a - 0x800) / 4) as usize),
            0xC00..=0xC17 => self.cfg_word(cur, ((a - 0xC00) / 4) as usize),
            0xF10..=0xF2F => self.sgi_word(cur, ((a - 0xF10) % 0x10 / 4) as usize),
            _ => 0,
        }
    }

    fn enable_word(&self, cur: usize, n: usize) -> u32 {
        (0..32)
            .filter(|j| {
                let id = 32 * n + j;
                id < NUM_IRQS && self.line(cur, id).enabled
            })
            .fold(0, |w, j| w | (1 << j))
    }

    fn pending_word(&self, cur: usize, n: usize) -> u32 {
        (0..32)
            .filter(|j| {
                let id = 32 * n + j;
                id < NUM_IRQS && self.pending_raw(cur, id)
            })
            .fold(0, |w, j| w | (1 << j))
    }

    fn active_word(&self, n: usize) -> u32 {
        (0..32)
            .filter(|j| self.is_active(32 * n + j))
            .fold(0, |w, j| w | (1 << j))
    }

    fn prio_word(&self, cur: usize, k: usize) -> u32 {
        (0..4).fold(0, |w, j| {
            let id = 4 * k + j;
            if id < NUM_IRQS {
                w | (u32::from(self.line(cur, id).prio) << (8 * j))
            } else {
                w
            }
        })
    }

    fn targets_word(&self, cur: usize, k: usize) -> u32 {
        (0..4).fold(0, |w, j| {
            let id = 4 * k + j;
            let t = if id < 32 {
                1 << cur // banked: the core reads its own bit
            } else if id < NUM_IRQS {
                self.targets[id - 32]
            } else {
                0
            };
            w | (u32::from(t) << (8 * j))
        })
    }

    fn cfg_word(&self, cur: usize, k: usize) -> u32 {
        (0..16).fold(0, |w, j| {
            let id = 16 * k + j;
            if id < NUM_IRQS && (id < 16 || self.line(cur, id).edge) {
                w | (2 << (2 * j))
            } else {
                w
            }
        })
    }

    fn sgi_word(&self, cur: usize, k: usize) -> u32 {
        (0..4).fold(0, |w, j| {
            w | (u32::from(self.sgi[cur][4 * k + j]) << (8 * j))
        })
    }

    /// Writes an aligned distributor word. Unwritten lanes of `w` already hold the register's
    /// current value (read-write registers) or zero (set and clear registers).
    fn dist_write_word(&mut self, a: u64, w: u32) {
        let cur = self.cur;
        match a {
            0x000 => self.dist_enabled = w & 1 != 0,
            0x100..=0x10F | 0x180..=0x18F => {
                let on = a < 0x180;
                let n = word_index(a);
                for j in 0..32 {
                    let id = 32 * n + j;
                    if w & (1 << j) != 0 && id < NUM_IRQS {
                        self.line_mut(cur, id).enabled = on;
                    }
                }
            }
            0x200..=0x20F | 0x280..=0x28F => {
                let on = a < 0x280;
                let n = word_index(a);
                for j in 0..32 {
                    let id = 32 * n + j;
                    // SGIs are pending through ICDSGIR and the SGI registers only.
                    if w & (1 << j) != 0 && (16..NUM_IRQS).contains(&id) {
                        self.line_mut(cur, id).sw = on;
                    }
                }
            }
            0x400..=0x5FF => {
                let k = ((a - 0x400) / 4) as usize;
                for j in 0..4 {
                    let id = 4 * k + j;
                    if id < NUM_IRQS {
                        self.line_mut(cur, id).prio = (w >> (8 * j)) as u8;
                    }
                }
            }
            0x800..=0x9FF => {
                let k = ((a - 0x800) / 4) as usize;
                for j in 0..4 {
                    let id = 4 * k + j;
                    // SGI and PPI targets are read-only (banked).
                    if (32..NUM_IRQS).contains(&id) {
                        self.targets[id - 32] = (w >> (8 * j)) as u8;
                    }
                }
            }
            0xC00..=0xC17 => {
                let k = ((a - 0xC00) / 4) as usize;
                for j in 0..16 {
                    let id = 16 * k + j;
                    if (16..NUM_IRQS).contains(&id) {
                        // Bit 2j+1 of the field selects edge; the SGI fields are fixed.
                        self.line_mut(cur, id).edge = w & (2 << (2 * j)) != 0;
                    }
                }
            }
            0xF00 => self.software_interrupt(w),
            0xF10..=0xF1F => {
                let k = ((a - 0xF10) / 4) as usize;
                for j in 0..4 {
                    self.sgi[cur][4 * k + j] &= !((w >> (8 * j)) as u8);
                }
            }
            0xF20..=0xF2F => {
                let k = ((a - 0xF20) / 4) as usize;
                for j in 0..4 {
                    self.sgi[cur][4 * k + j] |= (w >> (8 * j)) as u8;
                }
            }
            _ => {}
        }
    }

    /// ICDSGIR: sends SGI `w & 0xF` from the selected core to the cores chosen by the filter
    /// (bits 25:24) and the target list (bits 23:16).
    fn software_interrupt(&mut self, w: u32) {
        let id = (w & 0xF) as usize;
        let all = ((1u16 << NCPU) - 1) as u8;
        let me = 1u8 << self.cur;
        let targets = match (w >> 24) & 3 {
            0 => ((w >> 16) as u8) & all,
            1 => all & !me,
            2 => me,
            _ => 0,
        };
        for t in 0..NCPU {
            if targets & (1 << t) != 0 {
                self.sgi[t][id] |= me;
            }
        }
    }

    // ---- CPU interface -----------------------------------------------------------------

    /// Whether a CPU interface register is read-write.
    fn cpu_is_rw(a: u64) -> bool {
        matches!(a, 0x00 | 0x04 | 0x08 | 0x1C)
    }

    /// Word value of a CPU interface register. Reading ICCIAR acknowledges an interrupt.
    fn cpu_word(&mut self, a: u64) -> u32 {
        let cur = self.cur;
        match a {
            0x00 => u32::from(self.cpu_enabled[cur]),
            0x04 => u32::from(self.pmr[cur]),
            0x08 => u32::from(self.bpr[cur]),
            0x0C => self.ack(cur),
            0x14 => u32::from(u8::try_from(self.running(cur)).unwrap_or(0xFF)),
            0x18 => self.highest_pending(cur),
            0x1C => u32::from(self.abpr[cur]),
            0xFC => IIDR,
            _ => 0,
        }
    }

    fn cpu_write_word(&mut self, a: u64, w: u32) {
        let cur = self.cur;
        match a {
            0x00 => self.cpu_enabled[cur] = w & 1 != 0,
            0x04 => self.pmr[cur] = w as u8,
            0x08 => self.bpr[cur] = (w & 7) as u8,
            0x10 => self.eoi(cur, w),
            0x1C => self.abpr[cur] = (w & 7) as u8,
            _ => {}
        }
    }

    // ---- bus-facing accessors ----------------------------------------------------------

    /// Reads `size` bytes at `offset` of the distributor.
    pub fn read_dist(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let w = self.dist_word(offset & !3);
        lane_read(offset, size, w)
    }

    /// Writes `size` bytes at `offset` of the distributor.
    pub fn write_dist(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        let cur = if Self::dist_is_rw(a) {
            self.dist_word(a)
        } else {
            0
        };
        let (a, w) = lane_merge(offset, size, value, cur)?;
        self.dist_write_word(a, w);
        Ok(())
    }

    /// Reads `size` bytes at `offset` of the CPU interface of the selected core.
    pub fn read_cpu(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let w = self.cpu_word(offset & !3);
        lane_read(offset, size, w)
    }

    /// Writes `size` bytes at `offset` of the CPU interface of the selected core.
    pub fn write_cpu(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        let cur = if Self::cpu_is_rw(a) {
            self.cpu_word_readback(a)
        } else {
            0
        };
        let (a, w) = lane_merge(offset, size, value, cur)?;
        self.cpu_write_word(a, w);
        Ok(())
    }

    /// The readback of a read-write CPU interface register, without side effects.
    fn cpu_word_readback(&self, a: u64) -> u32 {
        let cur = self.cur;
        match a {
            0x00 => u32::from(self.cpu_enabled[cur]),
            0x04 => u32::from(self.pmr[cur]),
            0x08 => u32::from(self.bpr[cur]),
            0x1C => u32::from(self.abpr[cur]),
            _ => 0,
        }
    }

    /// Serialises the controller state for determinism hashes.
    pub fn save_state(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.dist_enabled));
        for core in &self.own {
            for l in core {
                push_line(out, l);
            }
        }
        for core in &self.sgi {
            out.extend_from_slice(core);
        }
        for l in &self.shared {
            push_line(out, l);
        }
        out.extend_from_slice(&self.targets);
        for c in 0..NCPU {
            out.push(u8::from(self.cpu_enabled[c]));
            out.extend_from_slice(&[self.pmr[c], self.bpr[c], self.abpr[c]]);
            for &(id, p) in &self.active[c] {
                out.extend_from_slice(&id.to_le_bytes());
                out.push(p);
            }
            out.push(0xFF);
        }
    }

    /// Whether interrupt `id` is enabled and pending for core `cpu`.
    pub fn is_pending(&self, cpu: usize, id: u16) -> bool {
        self.pending_for(cpu, usize::from(id)) && self.line(cpu, usize::from(id)).enabled
    }
}

/// The index of a 32-bit register group: `(offset & 0x7F) / 4` covers the four words of the
/// ICDISER/ICDICER and ICDISPR/ICDICPR banks.
fn word_index(a: u64) -> usize {
    ((a & 0x7F) / 4) as usize
}

fn push_line(out: &mut Vec<u8>, l: &Line) {
    out.extend_from_slice(&[
        u8::from(l.enabled),
        l.prio,
        u8::from(l.edge),
        u8::from(l.line),
        u8::from(l.hw),
        u8::from(l.sw),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(id: u16, cpu: Option<u8>, on: bool) -> IrqInput {
        IrqInput { id, cpu, level: on }
    }

    /// Enables the distributor and core 0's CPU interface, and opens the priority mask.
    fn bring_up(g: &mut Gic) {
        g.write_dist(0x000, 4, 1).unwrap();
        g.write_cpu(0x00, 4, 1).unwrap();
        g.write_cpu(0x04, 4, 0xF0).unwrap();
    }

    #[test]
    fn a_disabled_interrupt_is_not_signalled() {
        let mut g = Gic::new();
        bring_up(&mut g);
        g.accept(&[level(29, Some(0), true)]);
        assert!(!g.signal(0));
        assert_eq!(g.ack(0), SPURIOUS);
    }

    #[test]
    fn enabled_ppi_is_signalled_acknowledged_and_ended() {
        let mut g = Gic::new();
        bring_up(&mut g);
        g.write_dist(0x100, 4, 1 << 29).unwrap();
        g.write_dist(0x400 + 29, 1, 0xA0).unwrap();
        g.accept(&[level(29, Some(0), true)]);
        assert!(g.signal(0));
        assert!(!g.signal(1), "a PPI belongs to its core");
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 29);
        assert!(!g.signal(0), "active interrupt is not signalled again");
        g.write_cpu(0x10, 4, 29).unwrap();
        // The line is still high: level-sensitive, so it is pending again after EOI.
        assert!(g.signal(0));
        g.accept(&[level(29, Some(0), false)]);
        assert!(!g.signal(0));
    }

    #[test]
    fn priority_mask_and_running_priority_gate_delivery() {
        let mut g = Gic::new();
        bring_up(&mut g);
        // SPIs 40 and 41: enable bits 8 and 9 of ICDISER word 1, target core 0.
        g.write_dist(0x104, 4, (1 << 8) | (1 << 9)).unwrap();
        g.write_dist(0x800 + 40, 1, 0x01).unwrap();
        g.write_dist(0x800 + 41, 1, 0x01).unwrap();
        g.write_dist(0x400 + 40, 1, 0x80).unwrap();
        g.write_dist(0x400 + 41, 1, 0x40).unwrap();
        g.accept(&[level(40, None, true), level(41, None, true)]);
        g.write_cpu(0x04, 4, 0x20).unwrap(); // mask: only priorities below 0x20
        assert!(!g.signal(0));
        g.write_cpu(0x04, 4, 0xF0).unwrap();
        // The more urgent interrupt (0x40) is taken first.
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 41);
        assert!(!g.signal(0), "0x80 is less urgent than the running 0x40");
    }

    #[test]
    fn spi_goes_only_to_its_target_core() {
        let mut g = Gic::new();
        bring_up(&mut g);
        g.write_cpu(0x00, 4, 0).unwrap();
        g.set_cpu(1);
        g.write_cpu(0x00, 4, 1).unwrap();
        g.write_cpu(0x04, 4, 0xF0).unwrap();
        g.set_cpu(0);
        g.write_dist(0x100 + 4, 4, 1 << 10).unwrap(); // ICDISER word 1, bit 10: SPI 42
        g.write_dist(0x800 + 42, 1, 0x02).unwrap(); // target core 1 for SPI 42
        g.write_dist(0x400 + 42, 1, 0x90).unwrap();
        g.accept(&[level(42, None, true)]);
        assert!(!g.signal(0));
        g.set_cpu(1);
        assert!(g.signal(1));
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 42);
    }

    #[test]
    fn edge_input_latches_until_acknowledged() {
        let mut g = Gic::new();
        bring_up(&mut g);
        g.write_dist(0x104, 4, 1 << 0).unwrap(); // SPI 32 enabled (ICDISER word 1)
        g.write_dist(0x800 + 32, 1, 0x01).unwrap(); // target core 0
        g.write_dist(0x400 + 32, 1, 0x80).unwrap();
        // Edge: ICDICFR word 2 covers ids 32..47; SPI 32 is field 0, edge is its bit 1.
        g.write_dist(0xC00 + 8, 4, 2).unwrap();
        g.accept(&[level(32, None, true)]);
        g.accept(&[level(32, None, false)]);
        assert!(g.signal(0), "the rising edge stays pending");
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 32);
        assert!(!g.signal(0));
    }

    #[test]
    fn sgi_names_its_source_core_in_the_acknowledge() {
        let mut g = Gic::new();
        bring_up(&mut g);
        g.set_cpu(1);
        g.write_cpu(0x00, 4, 1).unwrap();
        g.write_cpu(0x04, 4, 0xF0).unwrap();
        g.write_dist(0x100, 4, 1 << 3).unwrap();
        g.write_dist(0x400 + 3, 1, 0x10).unwrap();
        g.set_cpu(0);
        // Core 0 sends SGI 3 to core 1 (target list bit 1).
        g.write_dist(0xF00, 4, (0b10 << 16) | 3).unwrap();
        g.set_cpu(1);
        assert!(g.signal(1));
        let iar = g.read_cpu(0x0C, 4).unwrap();
        assert_eq!(iar & 0x3FF, 3);
        assert_eq!((iar >> 10) & 7, 0, "source core 0");
    }

    #[test]
    fn sgi_filters_pick_the_cores_and_priorities_order_sgi_against_a_ppi() {
        let mut g = Gic::new();
        g.write_dist(0x000, 4, 1).unwrap();
        for core in [0usize, 1] {
            g.set_cpu(core);
            g.write_cpu(0x00, 4, 1).unwrap();
            g.write_cpu(0x04, 4, 0xF0).unwrap();
            // SGI 5 and PPI 29 enabled for this core; SGI 5 at 0x60, PPI 29 at 0x20.
            g.write_dist(0x100, 4, (1 << 5) | (1 << 29)).unwrap();
            g.write_dist(0x400 + 5, 1, 0x60).unwrap();
            g.write_dist(0x400 + 29, 1, 0x20).unwrap();
        }
        // Core 0 sends SGI 5 to all other CPUs (filter 0b01): core 1 only.
        g.set_cpu(0);
        g.write_dist(0xF00, 4, (0b01 << 24) | 5).unwrap();
        assert!(
            !g.signal(0),
            "the 'all other CPUs' filter excludes the sender"
        );
        assert!(g.signal(1));
        // The PPI is more urgent than the SGI, so it is acknowledged first.
        g.accept(&[level(29, Some(1), true)]);
        g.set_cpu(1);
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 29);
        // The device drops its level, then the handler ends the PPI.
        g.accept(&[level(29, Some(1), false)]);
        g.write_cpu(0x10, 4, 29).unwrap();
        let iar = g.read_cpu(0x0C, 4).unwrap();
        assert_eq!(iar & 0x3FF, 5, "then the SGI");
        assert_eq!((iar >> 10) & 7, 0, "from core 0");
        g.write_cpu(0x10, 4, iar).unwrap();
        // The self filter (0b10) delivers to the sender alone.
        g.set_cpu(0);
        g.write_dist(0xF00, 4, (0b10 << 24) | 5).unwrap();
        assert!(g.signal(0));
        assert_eq!(g.read_cpu(0x0C, 4).unwrap() & 0x3FF, 5);
        // A target list naming no core sends nothing.
        g.write_dist(0xF00, 4, 5).unwrap();
        assert!(!g.signal(0) && !g.signal(1));
    }

    #[test]
    fn register_readback_and_byte_lanes() {
        let mut g = Gic::new();
        g.write_dist(0x400 + 33, 1, 0xA5).unwrap();
        assert_eq!(g.read_dist(0x400 + 32, 4).unwrap(), 0x0000_A500);
        assert_eq!(g.read_dist(0x004, 4).unwrap(), 0x22);
        assert_eq!(g.read_dist(0x800 + 32, 1).unwrap(), 0, "no targets set yet");
        // SGI and PPI targets read back as the selected core's bit.
        assert_eq!(g.read_dist(0x800, 1).unwrap(), 1);
        g.set_cpu(1);
        assert_eq!(g.read_dist(0x800 + 29, 1).unwrap(), 2);
        assert!(
            g.write_dist(0x401, 2, 0).is_err(),
            "half-word at an odd offset is refused"
        );
    }

    #[test]
    fn highest_pending_and_idle_values() {
        let mut g = Gic::new();
        bring_up(&mut g);
        assert_eq!(g.read_cpu(0x18, 4).unwrap(), 1023);
        assert_eq!(g.read_cpu(0x14, 4).unwrap(), 0xFF);
        assert_eq!(g.read_cpu(0x0C, 4).unwrap(), 1023, "nothing to acknowledge");
    }
}
