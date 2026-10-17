//! Cadence-style GPIO controller (xgpiops, UG585 ch.14) at 0xE000A000, interrupt line GIC ID 52.
//!
//! Four banks: bank 0 = MIO[15:0], bank 1 = MIO[31:16], bank 2 = MIO[53:32] (22 pins), bank 3 =
//! EMIO[31:0] (pins 54..=85). The register layout is the one `firmware/conf_gpiops` uses:
//! masked data writes at 0x000 + 8n (LSW) and 0x004 + 8n (MSW), DATA_n at 0x040 + 4n, DATA_RO_n
//! at 0x060 + 4n, and the bank block at 0x204 + 0x40n: DIRM 0x00, OEN 0x04, INT_MASK 0x08,
//! INT_EN 0x0C (write 1 clears the mask bit), INT_DIS 0x10 (write 1 sets it), INT_STAT 0x14
//! (write 1 clears), INT_TYPE 0x18, INT_POLARITY 0x1C, INT_ANY 0x20.
//!
//! Deviation from the request: the request listed pins 86..=117 as a fourth EMIO range. The
//! register set has four banks of at most 32 bits, so pins 0..=85 are the modelled range; a
//! pin above 85 is refused by [`Gpio::set_input`] and reads as 0 in DATA_RO. `VERIFY:` the
//! bank widths (16, 16, 22, 32) and the EMIO numbering against UG585.
//!
//! Pin model. A pin is driven by the PS when both DIRM and OEN are set; then its level is the
//! DATA bit. Otherwise it follows the host's input (`set_input`). DATA_RO reads the level of
//! every pin. The output sink is the log of driven pins whose level changed
//! ([`Gpio::take_output_log`]).
//!
//! Interrupts. A change of a pin's level is an event: an edge type (INT_TYPE = 1) sets the
//! pin's status bit on any change (INT_ANY = 1) or on the edge the polarity bit selects (1 =
//! rising, 0 = falling); a level type sets it when the level enters the polarity bit's value.
//! The status is latched until written 1 (`VERIFY:` the latch for level types, which the
//! firmware check requires not to set without an event). The single line to the GIC (ID 52) is
//! asserted while any unmasked status bit is set.

use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};

/// GIC interrupt ID of the GPIO block (SPI 52).
pub const GPIO_IRQ: u16 = 52;
/// Pins the model has (MIO 0..=53 and EMIO 54..=85).
pub const PINS: usize = 86;
/// Valid pin bits per bank (VERIFY: UG585 bank widths 16, 16, 22, 32).
pub const BANK_WIDTH: [u32; 4] = [0xFFFF, 0xFFFF, 0x003F_FFFF, 0xFFFF_FFFF];
/// First pin of each bank.
const BANK_FIRST: [usize; 4] = [0, 16, 32, 54];
/// Bound of the output log, so that a host that never drains it cannot grow it without limit.
pub const OUTPUT_LOG_CAP: usize = 4096;

const MASK_DATA_BASE: u64 = 0x000;
const DATA_BASE: u64 = 0x040;
const DATA_RO_BASE: u64 = 0x060;
const BANK_BASE: u64 = 0x204;
const BANK_STRIDE: u64 = 0x40;

const DIRM: u64 = 0x00;
const OEN: u64 = 0x04;
const INT_MASK: u64 = 0x08;
const INT_EN: u64 = 0x0C;
const INT_DIS: u64 = 0x10;
const INT_STAT: u64 = 0x14;
const INT_TYPE: u64 = 0x18;
const INT_POLARITY: u64 = 0x1C;
const INT_ANY: u64 = 0x20;

/// The bank a pin belongs to, if the model has the pin.
pub fn bank_of(pin: usize) -> Option<usize> {
    if pin >= PINS {
        return None;
    }
    (0..4).rev().find(|&b| pin >= BANK_FIRST[b])
}

/// Merges a masked data write into `old` for the 16-bit half selected by `msw`.
///
/// The written word's bits 31:16 are the inhibit mask (1 = keep the old pin), bits 15:0 the
/// data. The LSW register covers bank bits 15:0, the MSW register bits 31:16.
fn masked_merge(old: u32, value: u64, msw: bool) -> u32 {
    let shift = if msw { 16 } else { 0 };
    let region = 0xFFFFu32 << shift;
    let inhibit = (((value >> 16) as u32) & 0xFFFF) << shift;
    let data = (((value as u32) & 0xFFFF) << shift) & region;
    (old & (inhibit | !region)) | (data & !inhibit)
}

/// The GPIO controller.
pub struct Gpio {
    /// DATA_n: the output data register of each bank.
    data: [u32; 4],
    dirm: [u32; 4],
    oen: [u32; 4],
    /// INT_MASK: 1 = masked.
    mask: [u32; 4],
    int_type: [u32; 4],
    polarity: [u32; 4],
    any: [u32; 4],
    /// INT_STAT: latched status bits.
    stat: [u32; 4],
    /// The host's level for every pin (what an input pin reads).
    input: [u32; 4],
    /// The effective level of every pin at the last refresh.
    last: [u32; 4],
    out_log: Vec<(u16, bool)>,
}

impl Default for Gpio {
    fn default() -> Self {
        Gpio::new()
    }
}

impl Gpio {
    /// The controller in reset state: all pins inputs at 0, every interrupt masked.
    pub fn new() -> Gpio {
        let mut g = Gpio {
            data: [0; 4],
            dirm: [0; 4],
            oen: [0; 4],
            mask: [0; 4],
            int_type: [0; 4],
            polarity: [0; 4],
            any: [0; 4],
            stat: [0; 4],
            input: [0; 4],
            last: [0; 4],
            out_log: Vec::new(),
        };
        g.mask = BANK_WIDTH;
        g
    }

    /// Drives the host level of pin `pin` (an input pin reads it; a driven pin ignores it).
    /// Returns false for a pin the model does not have.
    pub fn set_input(&mut self, pin: usize, level: bool) -> bool {
        let Some(b) = bank_of(pin) else {
            return false;
        };
        let bit = 1u32 << (pin - BANK_FIRST[b]);
        if level {
            self.input[b] |= bit;
        } else {
            self.input[b] &= !bit;
        }
        self.refresh();
        true
    }

    /// The level of pin `pin` as the pad shows it (DATA_RO), `None` for a pin the model lacks.
    pub fn pin_level(&self, pin: usize) -> Option<bool> {
        let b = bank_of(pin)?;
        Some(self.effective(b) >> (pin - BANK_FIRST[b]) & 1 != 0)
    }

    /// Whether the PS drives pin `pin` (DIRM and OEN both set).
    pub fn is_driven(&self, pin: usize) -> bool {
        bank_of(pin).is_some_and(|b| {
            let bit = 1u32 << (pin - BANK_FIRST[b]);
            (self.dirm[b] & self.oen[b]) & bit != 0
        })
    }

    /// Takes the output sink: every driven pin whose level changed since the last call, in
    /// order, as `(pin, level)`. At most [`OUTPUT_LOG_CAP`] entries are kept.
    pub fn take_output_log(&mut self) -> Vec<(u16, bool)> {
        std::mem::take(&mut self.out_log)
    }

    /// The effective level of bank `b`: the DATA bit where the pin is driven, the host's level
    /// elsewhere.
    fn effective(&self, b: usize) -> u32 {
        let drive = self.dirm[b] & self.oen[b];
        ((self.data[b] & drive) | (self.input[b] & !drive)) & BANK_WIDTH[b]
    }

    /// Recomputes the pad levels and turns each change into interrupt status and output log
    /// entries.
    fn refresh(&mut self) {
        for (b, &first) in BANK_FIRST.iter().enumerate() {
            let now = self.effective(b);
            let changed = now ^ self.last[b];
            let drive = self.dirm[b] & self.oen[b];
            for i in 0..32u32 {
                let bit = 1u32 << i;
                if changed & bit == 0 {
                    continue;
                }
                let level = now & bit != 0;
                let pin = first + i as usize;
                if drive & bit != 0 && self.out_log.len() < OUTPUT_LOG_CAP {
                    self.out_log.push((pin as u16, level));
                }
                let pol = self.polarity[b] & bit != 0;
                let event = if self.int_type[b] & bit != 0 {
                    self.any[b] & bit != 0 || pol == level
                } else {
                    pol == level
                };
                if event {
                    self.stat[b] |= bit;
                }
            }
            self.last[b] = now;
        }
    }

    fn level_irq(&self) -> bool {
        (0..4).any(|b| self.stat[b] & !self.mask[b] & BANK_WIDTH[b] != 0)
    }

    /// Decodes a bank register: the bank index and the register offset within the bank block.
    fn bank_reg(offset: u64) -> Option<(usize, u64)> {
        let idx = offset.checked_sub(BANK_BASE)?;
        let (b, k) = (idx / BANK_STRIDE, idx % BANK_STRIDE);
        (b < 4 && k <= INT_ANY && k % 4 == 0).then_some((b as usize, k))
    }

    fn bank_word(&self, b: usize, k: u64) -> u32 {
        match k {
            DIRM => self.dirm[b],
            OEN => self.oen[b],
            INT_MASK => self.mask[b],
            INT_STAT => self.stat[b],
            INT_TYPE => self.int_type[b],
            INT_POLARITY => self.polarity[b],
            INT_ANY => self.any[b],
            _ => 0,
        }
    }
}

impl Device for Gpio {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let a = offset & !3;
        let word = if let Some((b, k)) = Gpio::bank_reg(a) {
            self.bank_word(b, k)
        } else if (DATA_BASE..DATA_BASE + 16).contains(&a) {
            let b = ((a - DATA_BASE) / 4) as usize;
            self.data[b] & BANK_WIDTH[b]
        } else if (DATA_RO_BASE..DATA_RO_BASE + 16).contains(&a) {
            let b = ((a - DATA_RO_BASE) / 4) as usize;
            self.effective(b)
        } else if (MASK_DATA_BASE..MASK_DATA_BASE + 32).contains(&a) {
            // Masked data registers are write-only.
            0
        } else if a < 0x1000 {
            0
        } else {
            return Err(BusFault::Unmapped(offset));
        };
        lane_read(offset, size, word)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        if let Some((b, k)) = Gpio::bank_reg(a) {
            let w = BANK_WIDTH[b];
            let (_, v) = lane_merge(offset, size, value, 0)?;
            let v = v & w;
            match k {
                DIRM => self.dirm[b] = v,
                OEN => self.oen[b] = v,
                INT_MASK => {
                    // Read-only register: the mask moves through INT_EN and INT_DIS.
                    lane_merge(offset, size, value, self.mask[b])?;
                }
                INT_EN => self.mask[b] &= !v,
                INT_DIS => self.mask[b] = (self.mask[b] | v) & w,
                INT_STAT => self.stat[b] &= !v,
                INT_TYPE => self.int_type[b] = v,
                INT_POLARITY => self.polarity[b] = v,
                INT_ANY => self.any[b] = v,
                _ => {}
            }
        } else if (MASK_DATA_BASE..MASK_DATA_BASE + 32).contains(&a) {
            let rel = a - MASK_DATA_BASE;
            let b = (rel / 8) as usize;
            let msw = rel % 8 == 4;
            let (_, v) = lane_merge(offset, size, value, 0)?;
            self.data[b] = masked_merge(self.data[b], u64::from(v), msw) & BANK_WIDTH[b];
        } else if (DATA_BASE..DATA_BASE + 16).contains(&a) {
            let b = ((a - DATA_BASE) / 4) as usize;
            let (_, v) = lane_merge(offset, size, value, self.data[b])?;
            self.data[b] = v & BANK_WIDTH[b];
        } else if (DATA_RO_BASE..DATA_RO_BASE + 16).contains(&a) {
            // Read-only: the write is accepted and ignored.
            lane_merge(offset, size, value, 0)?;
        } else if a < 0x1000 {
            lane_merge(offset, size, value, 0)?;
        } else {
            return Err(BusFault::Unmapped(offset));
        }
        self.refresh();
        Ok(())
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        out.push(IrqInput {
            id: GPIO_IRQ,
            cpu: None,
            level: self.level_irq(),
        });
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for arr in [
            &self.data,
            &self.dirm,
            &self.oen,
            &self.mask,
            &self.int_type,
            &self.polarity,
            &self.any,
            &self.stat,
            &self.input,
            &self.last,
        ] {
            for v in arr {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for (pin, level) in &self.out_log {
            out.extend_from_slice(&pin.to_le_bytes());
            out.push(u8::from(*level));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wr(g: &mut Gpio, off: u64, v: u32) {
        g.write(off, 4, u64::from(v)).unwrap();
    }

    fn rd(g: &mut Gpio, off: u64) -> u32 {
        g.read(off, 4).unwrap() as u32
    }

    #[test]
    fn reset_values_are_inputs_low_and_fully_masked() {
        let mut g = Gpio::new();
        for b in 0..4u64 {
            assert_eq!(rd(&mut g, BANK_BASE + 0x40 * b + DIRM), 0);
            assert_eq!(rd(&mut g, BANK_BASE + 0x40 * b + OEN), 0);
            assert_eq!(
                rd(&mut g, BANK_BASE + 0x40 * b + INT_MASK),
                BANK_WIDTH[b as usize]
            );
            assert_eq!(rd(&mut g, BANK_BASE + 0x40 * b + INT_TYPE), 0);
        }
    }

    #[test]
    fn masked_lsw_and_msw_writes_keep_inhibited_pins() {
        let mut g = Gpio::new();
        wr(&mut g, DATA_BASE, 0x5A5A);
        wr(&mut g, MASK_DATA_BASE, (0x00FF << 16) | 0x3C00);
        assert_eq!(rd(&mut g, DATA_BASE), 0x3C5A);
        wr(&mut g, DATA_BASE + 12, 0xA5A5_5A5A);
        wr(&mut g, MASK_DATA_BASE + 8 * 3 + 4, (0x00FF << 16) | 0x3C00);
        assert_eq!(rd(&mut g, DATA_BASE + 12), 0x3CA5_5A5A);
    }

    #[test]
    fn data_and_direction_registers_keep_what_is_written() {
        let mut g = Gpio::new();
        wr(&mut g, BANK_BASE + DIRM, 0xA5C3_5A3C);
        assert_eq!(rd(&mut g, BANK_BASE + DIRM), 0x5A3C, "bank 0 is 16 pins");
        wr(&mut g, DATA_BASE + 8, 0xFFFF_FFFF);
        assert_eq!(rd(&mut g, DATA_BASE + 8), 0x003F_FFFF, "bank 2 is 22 pins");
    }

    #[test]
    fn driven_pin_reads_its_data_and_follows_the_host_when_not_driven() {
        let mut g = Gpio::new();
        g.set_input(3, true);
        assert_eq!(g.pin_level(3), Some(true));
        wr(&mut g, BANK_BASE + DIRM, 1 << 3);
        wr(&mut g, BANK_BASE + OEN, 1 << 3);
        wr(&mut g, DATA_BASE, 0);
        assert_eq!(g.pin_level(3), Some(false), "driven low by DATA");
        wr(&mut g, BANK_BASE + OEN, 0);
        assert_eq!(g.pin_level(3), Some(true), "not driven: the host level");
        assert!(!g.is_driven(3));
    }

    #[test]
    fn output_sink_records_changes_of_driven_pins_only() {
        let mut g = Gpio::new();
        wr(&mut g, BANK_BASE + DIRM, 0b11);
        wr(&mut g, BANK_BASE + OEN, 0b11);
        wr(&mut g, DATA_BASE, 0b01);
        wr(&mut g, DATA_BASE, 0b01);
        g.set_input(5, true);
        assert_eq!(
            g.take_output_log(),
            vec![(0, true)],
            "pin 1 never changes; pin 5 is not driven"
        );
        assert!(g.take_output_log().is_empty(), "drained");
    }

    #[test]
    fn edge_interrupt_follows_polarity_and_any() {
        let mut g = Gpio::new();
        // Pin 20 (bank 1), edge, rising: INT_EN unmasks bank 1 bit 4.
        wr(&mut g, BANK_BASE + 0x40 + INT_TYPE, 1 << 4);
        wr(&mut g, BANK_BASE + 0x40 + INT_POLARITY, 1 << 4);
        wr(&mut g, BANK_BASE + 0x40 + INT_EN, 1 << 4);
        g.set_input(20, false);
        assert_eq!(
            rd(&mut g, BANK_BASE + 0x40 + INT_STAT),
            0,
            "falling is not selected"
        );
        g.set_input(20, true);
        assert_eq!(rd(&mut g, BANK_BASE + 0x40 + INT_STAT), 1 << 4);
        let mut lines = Vec::new();
        g.irq_lines(&mut lines);
        assert_eq!(
            lines,
            vec![IrqInput {
                id: 52,
                cpu: None,
                level: true
            }]
        );
        wr(&mut g, BANK_BASE + 0x40 + INT_STAT, 1 << 4);
        assert_eq!(rd(&mut g, BANK_BASE + 0x40 + INT_STAT), 0, "write 1 clears");
        // INT_ANY: both edges count.
        wr(&mut g, BANK_BASE + 0x40 + INT_ANY, 1 << 4);
        g.set_input(20, false);
        assert_eq!(rd(&mut g, BANK_BASE + 0x40 + INT_STAT), 1 << 4);
    }

    #[test]
    fn level_interrupt_sets_on_entering_the_polarity_level_and_masking_drops_the_line() {
        let mut g = Gpio::new();
        wr(&mut g, BANK_BASE + INT_POLARITY, 1); // pin 0, level high
        wr(&mut g, BANK_BASE + INT_EN, 1);
        let mut lines = Vec::new();
        g.set_input(0, true);
        g.irq_lines(&mut lines);
        assert!(lines[0].level);
        wr(&mut g, BANK_BASE + INT_DIS, 1);
        lines.clear();
        g.irq_lines(&mut lines);
        assert!(
            !lines[0].level,
            "masked: the status stays but the line is low"
        );
        assert_eq!(rd(&mut g, BANK_BASE + INT_STAT) & 1, 1);
    }

    #[test]
    fn bank3_pins_are_emio_54_to_85_and_pin_86_is_refused() {
        let mut g = Gpio::new();
        assert_eq!(bank_of(53), Some(2));
        assert_eq!(bank_of(54), Some(3));
        assert_eq!(bank_of(85), Some(3));
        assert_eq!(bank_of(86), None);
        assert!(!g.set_input(86, true));
        assert_eq!(g.pin_level(86), None);
    }

    #[test]
    fn reserved_offsets_read_zero_and_far_offsets_fault() {
        let mut g = Gpio::new();
        assert_eq!(g.read(0x0A0, 4), Ok(0));
        assert_eq!(g.read(0x2E8, 4), Ok(0));
        assert!(g.read(0x1000, 4).is_err());
    }
}
