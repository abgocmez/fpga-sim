//! The TI ADC081S021 8-bit ADC behind the TEMT6000 phototransistor of a
//! Digilent Pmod ALS (SPI, read only).
//!
//! Pmod pin order of the eight levels: bit0 CS (active low), bit1 not
//! connected, bit2 SDO (MISO), bit3 SCLK. `VERIFY:` against the reference
//! manual.
//!
//! One conversion is one 16-clock frame while CS is low: the output shifts
//! MSB first as `0000 DDDD DDDD 0000` (leading zeros, the 8 data bits, four
//! trailing zeros), i.e. the 16-bit word is `code << 4`. The conversion
//! result is taken when CS falls. Not modelled: the tri-state first bit
//! (shown as a zero), power-down mode, conversion time, reference voltage:
//! the host sets the 8-bit code directly (`light`, 0..=255 counts).

use crate::spi::{SpiDevice, SpiSlave};
use crate::{BoardDevice, DeviceImage, DeviceRegister, DeviceValue, PinDrive, readout};

const CS_BIT: u8 = 0;
const MISO_BIT: u8 = 2;
const SCLK_BIT: u8 = 3;

#[derive(Clone, Debug)]
struct Adc {
    /// What the host set.
    code: u8,
    /// The code latched at the last CS fall.
    latched: u8,
    /// The frame's second byte.
    low: u8,
    conversions: u32,
}

impl SpiDevice for Adc {
    fn select(&mut self) -> u8 {
        self.latched = self.code;
        self.conversions += 1;
        let frame = u16::from(self.latched) << 4;
        self.low = frame as u8;
        (frame >> 8) as u8
    }

    fn exchange(&mut self, _mosi: u8) -> u8 {
        // The first exchange returns the low byte of the frame, later ones zeros.
        std::mem::take(&mut self.low)
    }
}

/// The ADC081S021 on a Pmod ALS.
#[derive(Clone, Debug)]
pub struct Adc081 {
    spi: SpiSlave,
    adc: Adc,
}

impl Default for Adc081 {
    fn default() -> Adc081 {
        Adc081::new()
    }
}

impl Adc081 {
    /// A converter reading code 0 (dark).
    pub fn new() -> Adc081 {
        Adc081 {
            // MOSI is not wired on the Pmod ALS; any unused bit will do.
            spi: SpiSlave::new(CS_BIT, 1, MISO_BIT, SCLK_BIT, 0),
            adc: Adc {
                code: 0,
                latched: 0,
                low: 0,
                conversions: 0,
            },
        }
    }

    /// The 8-bit code the next conversion returns.
    pub fn code(&self) -> u8 {
        self.adc.code
    }

    /// Conversions started (CS falls).
    pub fn conversions(&self) -> u32 {
        self.adc.conversions
    }
}

impl BoardDevice for Adc081 {
    fn kind(&self) -> &'static str {
        "als"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        self.spi.pins(&mut self.adc, levels);
    }

    fn drive(&self) -> PinDrive {
        self.spi.drive()
    }

    fn image(&self) -> Option<DeviceImage> {
        Some(readout(&[
            "ALS ADC081S021",
            &format!("LIGHT {}", self.adc.code),
            &format!("{} CONVERSIONS", self.adc.conversions),
        ]))
    }

    fn describe(&self) -> String {
        format!(
            "ADC081S021 (Pmod ALS) SPI, light code {} (last conversion {}), {} conversion(s)",
            self.adc.code, self.adc.latched, self.adc.conversions
        )
    }

    fn values(&self) -> Vec<DeviceValue> {
        vec![DeviceValue {
            name: "light",
            value: f64::from(self.adc.code),
            unit: "counts",
            min: 0.0,
            max: 255.0,
        }]
    }

    fn set_value(&mut self, name: &str, value: f64) -> Result<(), String> {
        if !name.eq_ignore_ascii_case("light") {
            return Err(format!("`als` has no quantity `{name}`; available: light"));
        }
        self.adc.code = value.round().clamp(0.0, 255.0) as u8;
        Ok(())
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        vec![
            DeviceRegister {
                name: "CODE".to_string(),
                addr: 0,
                value: u32::from(self.adc.code),
            },
            DeviceRegister {
                name: "LATCHED".to_string(),
                addr: 1,
                value: u32::from(self.adc.latched),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_checked;
    use crate::spi::tests::Master;

    fn frame(m: &mut Master<Adc>) -> u16 {
        m.select();
        let hi = m.byte(0);
        let lo = m.byte(0);
        m.deselect();
        u16::from(hi) << 8 | u16::from(lo)
    }

    fn adc(code: u8) -> Adc {
        Adc {
            code,
            latched: 0,
            low: 0,
            conversions: 0,
        }
    }

    #[test]
    fn the_frame_is_leading_zeros_data_and_trailing_zeros() {
        for mode in [0u8, 3] {
            let mut m = Master::new(adc(0xA7), mode);
            let f = frame(&mut m);
            assert_eq!(f, 0x0A70, "mode {mode}: 0000 1010 0111 0000");
            assert_eq!(f >> 4 & 0xFF, 0xA7);
            assert_eq!(f >> 12, 0, "leading zeros");
            assert_eq!(f & 0xF, 0, "trailing zeros");
        }
    }

    #[test]
    fn every_code_round_trips() {
        let mut m = Master::new(adc(0), 0);
        for code in [0u8, 1, 0x7F, 0x80, 0xFE, 0xFF] {
            m.dev.code = code;
            assert_eq!(frame(&mut m) >> 4, u16::from(code));
        }
        assert_eq!(m.dev.conversions, 6);
    }

    #[test]
    fn extra_clocks_return_zeros() {
        let mut m = Master::new(adc(0xFF), 0);
        m.select();
        let b: Vec<u8> = (0..4).map(|_| m.byte(0)).collect();
        m.deselect();
        assert_eq!(b, vec![0x0F, 0xF0, 0, 0]);
    }

    #[test]
    fn the_value_is_set_through_the_board_device() {
        let mut d = Adc081::new();
        assert_eq!(set_checked(&mut d, "light", 100.4), Ok(100.4));
        assert_eq!(d.code(), 100);
        assert_eq!(set_checked(&mut d, "LIGHT", 999.0), Ok(255.0));
        assert_eq!(d.code(), 255);
        assert!(set_checked(&mut d, "temperature", 1.0).is_err());
        assert_eq!(d.drive(), PinDrive::NONE, "MISO is released while idle");
        assert!(d.image().is_some());
        assert!(d.describe().contains("255"));
    }

    #[test]
    fn the_device_decodes_pins_end_to_end() {
        let mut d = Adc081::new();
        d.adc.code = 0x5A;
        let (cs, sclk, miso) = (1u8, 1u8 << 3, 1u8 << 2);
        d.pins(0, cs);
        d.pins(1, 0); // CS low: the first bit is on MISO
        let mut word = 0u16;
        for _ in 0..16 {
            // Mode 0: sample on the rising edge, shift on the falling edge.
            d.pins(2, sclk);
            word = word << 1 | u16::from(d.drive().levels & miso != 0);
            d.pins(3, 0);
        }
        d.pins(4, cs);
        assert_eq!(word, 0x05A0);
    }
}
