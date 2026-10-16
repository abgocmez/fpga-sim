//! A Digilent Pmod 8LD: eight LEDs on the eight signal pins of a Pmod port.
//!
//! Pmod pin order of the eight levels: bit *n* drives LD*n* (pins 1, 2, 3, 4,
//! 7, 8, 9, 10). The LEDs are active high: a pin driven high lights its LED.
//! `VERIFY:` the pin order and polarity against the Digilent reference manual.
//!
//! The module has no inputs: it shows the levels the PL drives, and it drives
//! nothing back.

use crate::{BoardDevice, DeviceImage, DeviceRegister};

const W: usize = 128;
const H: usize = 32;
/// Centre-to-centre distance of the LEDs in pixels.
const PITCH: usize = 16;
const RADIUS: i32 = 6;

/// The 8LD on a Pmod port.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Led8 {
    /// The levels the PL drives now: bit n lights LD n.
    leds: u8,
    /// Pin changes reported so far.
    changes: u32,
}

impl Led8 {
    /// Eight LEDs, all off.
    pub fn new() -> Led8 {
        Led8::default()
    }

    /// The lit LEDs, bit n = LD n.
    pub fn leds(&self) -> u8 {
        self.leds
    }

    /// Pin changes the module has seen.
    pub fn changes(&self) -> u32 {
        self.changes
    }
}

impl BoardDevice for Led8 {
    fn kind(&self) -> &'static str {
        "8ld"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        self.leds = levels;
        self.changes += 1;
    }

    fn image(&self) -> Option<DeviceImage> {
        // LD n is centred at x = 8 + 16 n (the eight LEDs centred in 128 px).
        let margin = (W - 8 * PITCH) / 2 + PITCH / 2;
        let mut rgba = Vec::with_capacity(W * H * 4);
        for y in 0..H {
            for x in 0..W {
                let mut px = [6, 8, 14, 255];
                for n in 0..8 {
                    let dx = x as i32 - (margin + n * PITCH) as i32;
                    let dy = y as i32 - (H / 2) as i32;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= RADIUS * RADIUS {
                        px = if self.leds >> n & 1 != 0 {
                            [255, 200, 60, 255]
                        } else {
                            [90, 90, 100, 255]
                        };
                        break;
                    }
                }
                rgba.extend_from_slice(&px);
            }
        }
        Some(DeviceImage {
            width: W as u32,
            height: H as u32,
            rgba,
        })
    }

    fn describe(&self) -> String {
        format!(
            "Pmod 8LD: LD0..LD7 = {:08b} ({} lit), {} pin change(s)",
            self.leds,
            self.leds.count_ones(),
            self.changes
        )
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        vec![DeviceRegister {
            name: "LEDS".to_string(),
            addr: 0,
            value: u32::from(self.leds),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_checked;

    fn lit_pixels(img: &DeviceImage) -> usize {
        img.rgba
            .chunks(4)
            .filter(|p| p[0] > 200 && p[1] > 150)
            .count()
    }

    #[test]
    fn the_pins_are_the_leds_in_order() {
        let mut d = Led8::new();
        assert_eq!(d.leds(), 0);
        d.pins(0, 0b1010_0101);
        assert_eq!(d.leds(), 0b1010_0101, "bit n lights LD n");
        assert_eq!(d.registers()[0].value, 0xA5);
        assert_eq!(d.changes(), 1);
        d.pins(10, 0);
        assert_eq!(d.leds(), 0);
        assert_eq!(d.changes(), 2);
    }

    #[test]
    fn it_drives_nothing_back() {
        let mut d = Led8::new();
        d.pins(0, 0xFF);
        assert_eq!(d.drive(), crate::PinDrive::NONE);
        assert!(
            set_checked(&mut d, "leds", 1.0).is_err(),
            "no settable quantity"
        );
    }

    #[test]
    fn the_picture_draws_eight_leds_and_lights_the_set_ones() {
        let mut d = Led8::new();
        let off = d.image().unwrap();
        assert_eq!((off.width, off.height), (128, 32));
        assert_eq!(off.rgba.len(), 128 * 32 * 4);
        assert_eq!(lit_pixels(&off), 0, "all off");
        d.pins(0, 0x01);
        let one = lit_pixels(&d.image().unwrap());
        d.pins(1, 0xFF);
        let all = lit_pixels(&d.image().unwrap());
        assert!(one > 0, "LD0 is drawn");
        assert!(all > 8 * one / 2, "eight LEDs are lit at full");
        assert!(d.describe().contains("8 lit"));
    }
}
