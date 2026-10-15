use crate::regs::RegisterBlock;

/// The prototype's `my_math_ip`: an AXI4-Lite slave with
/// `reg0` (offset 0x0, read/write) and `reg1 = reg0 * 2` (offset 0x4, read
/// only, modulo 2^32). Other offsets read as zero and ignore writes, which is
/// what `rtl/my_math_ip.v` does inside its 4 KiB window.
#[derive(Clone, Debug, Default)]
pub struct MathIp {
    reg0: u32,
}

impl MathIp {
    /// A freshly reset block.
    pub fn new() -> MathIp {
        MathIp::default()
    }
}

impl RegisterBlock for MathIp {
    fn name(&self) -> &str {
        "my_math_ip"
    }

    fn size(&self) -> u64 {
        0x1000
    }

    fn read32(&mut self, offset: u64) -> u32 {
        match offset {
            0x0 => self.reg0,
            0x4 => self.reg0.wrapping_mul(2),
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        if offset == 0x0 {
            let mut bytes = self.reg0.to_le_bytes();
            let new = value.to_le_bytes();
            for (lane, b) in bytes.iter_mut().enumerate() {
                if strobe & (1 << lane) != 0 {
                    *b = new[lane];
                }
            }
            self.reg0 = u32::from_le_bytes(bytes);
        }
    }

    fn reset(&mut self) {
        self.reg0 = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_modulo_2_pow_32_and_honours_strobes() {
        let mut m = MathIp::new();
        for (v, want) in [
            (0u32, 0u32),
            (1, 2),
            (0x15, 0x2A),
            (0x1234_5678, 0x2468_ACF0),
            (0x7FFF_FFFF, 0xFFFF_FFFE),
            (0x8000_0000, 0),
            (0xFFFF_FFFF, 0xFFFF_FFFE),
        ] {
            m.write32(0, v, 0xF);
            assert_eq!(m.read32(0), v);
            assert_eq!(m.read32(4), want);
        }
        // Byte strobes: only lane 1 is written.
        m.write32(0, 0x1111_1111, 0xF);
        m.write32(0, 0xFFFF_FFFF, 0b0010);
        assert_eq!(m.read32(0), 0x1111_FF11);
        // Read-only reg1, unmapped offsets.
        m.write32(4, 0xDEAD_BEEF, 0xF);
        assert_eq!(m.read32(4), 0x1111_FF11u32.wrapping_mul(2));
        assert_eq!(m.read32(0x10), 0);
        m.reset();
        assert_eq!(m.read32(0), 0);
    }
}
