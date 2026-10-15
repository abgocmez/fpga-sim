use crate::def::BoardDef;
use serde::{Deserialize, Serialize};

/// The logic level of a pin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinLevel {
    /// Driven low.
    Low,
    /// Driven high.
    High,
    /// High impedance / undriven.
    #[default]
    Z,
}

impl PinLevel {
    /// `0`, `1` or `Z`.
    pub fn symbol(self) -> char {
        match self {
            PinLevel::Low => '0',
            PinLevel::High => '1',
            PinLevel::Z => 'Z',
        }
    }
}

/// The mutable, user-visible state of the board.
///
/// Vectors are sized from the [`BoardDef`] and indexed like the definition's
/// lists. LED brightness is a PWM duty in `0..=255`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardState {
    /// Slide switch positions.
    pub switches: Vec<bool>,
    /// Push button states (true = pressed).
    pub buttons: Vec<bool>,
    /// User LED duty cycles.
    pub leds: Vec<u8>,
    /// RGB LED duty cycles as `[r, g, b]`.
    pub rgb: Vec<[u8; 3]>,
    /// Pmod signal pin levels, one row of eight per port.
    pub pmod_pins: Vec<[PinLevel; 8]>,
}

impl BoardState {
    /// All inputs released, all LEDs off, all Pmod pins undriven.
    pub fn new(def: &BoardDef) -> BoardState {
        BoardState {
            switches: vec![false; def.switches.len()],
            buttons: vec![false; def.buttons.len()],
            leds: vec![0; def.leds.len()],
            rgb: vec![[0; 3]; def.rgb_leds.len()],
            pmod_pins: vec![[PinLevel::Z; 8]; def.pmods.len()],
        }
    }

    /// Resets to the power-on state.
    pub fn reset(&mut self) {
        self.switches.fill(false);
        self.buttons.fill(false);
        self.leds.fill(0);
        self.rgb.fill([0; 3]);
        self.pmod_pins.fill([PinLevel::Z; 8]);
    }

    /// The level of a named pin or control.
    ///
    /// Recognised names are the ids in the definition (`sw0`, `btn1`, `led2`),
    /// RGB channels as `<id>_r|g|b` (`led5_g`), and Pmod pins (`JA1`, `JA10`).
    /// LEDs read high when their duty is at least 128.
    pub fn pin_level(&self, def: &BoardDef, name: &str) -> Option<PinLevel> {
        let lvl = |b: bool| if b { PinLevel::High } else { PinLevel::Low };
        if let Some(i) = def.switches.iter().position(|s| s.id == name) {
            return Some(lvl(self.switches[i]));
        }
        if let Some(i) = def.buttons.iter().position(|b| b.id == name) {
            return Some(lvl(self.buttons[i]));
        }
        if let Some(i) = def.leds.iter().position(|l| l.id == name) {
            return Some(lvl(self.leds[i] >= 128));
        }
        for (i, l) in def.rgb_leds.iter().enumerate() {
            for (c, suffix) in ["_r", "_g", "_b"].iter().enumerate() {
                if name == format!("{}{suffix}", l.id) {
                    return Some(lvl(self.rgb[i][c] >= 128));
                }
            }
        }
        for (i, p) in def.pmods.iter().enumerate() {
            if let Some(j) = p.pin_names().iter().position(|n| n == name) {
                return Some(self.pmod_pins[i][j]);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_lookup_covers_each_control_kind() {
        let def = BoardDef::zybo_z7_20();
        let mut s = BoardState::new(&def);
        s.switches[2] = true;
        s.buttons[0] = true;
        s.leds[3] = 200;
        s.rgb[0][1] = 255;
        s.pmod_pins[0][4] = PinLevel::High;
        assert_eq!(s.pin_level(&def, "sw2"), Some(PinLevel::High));
        assert_eq!(s.pin_level(&def, "sw0"), Some(PinLevel::Low));
        assert_eq!(s.pin_level(&def, "btn0"), Some(PinLevel::High));
        assert_eq!(s.pin_level(&def, "led3"), Some(PinLevel::High));
        assert_eq!(s.pin_level(&def, "led5_g"), Some(PinLevel::High));
        assert_eq!(s.pin_level(&def, "led5_r"), Some(PinLevel::Low));
        assert_eq!(s.pin_level(&def, "JA7"), Some(PinLevel::High));
        assert_eq!(s.pin_level(&def, "JA1"), Some(PinLevel::Z));
        assert_eq!(s.pin_level(&def, "nope"), None);
    }

    #[test]
    fn reset_restores_power_on_state() {
        let def = BoardDef::zybo_z7_20();
        let mut s = BoardState::new(&def);
        s.switches[0] = true;
        s.leds[0] = 255;
        s.reset();
        assert_eq!(s, BoardState::new(&def));
    }
}
