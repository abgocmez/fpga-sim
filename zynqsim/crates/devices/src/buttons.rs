//! Host-driven inputs on a Pmod: the push buttons of a Pmod BTN and the
//! slide switches of a Pmod SWT (`btn` and `swt` kinds).
//!
//! Nothing on the board presses them: the host sets each one with `set_value`
//! (`pmod_set`, `--device-set JB.btn0=1`), and the module drives the pins
//! accordingly, active high (a pressed button or a closed switch pulls its pin
//! high; `VERIFY:` the polarity and pin order against the Digilent manuals).
//!
//! Pin map (bit n = pin n of the port, Pmod order 1, 2, 3, 4, 7, 8, 9, 10):
//! * `btn`: bits 0..=3 are BTN0..BTN3 (`btn0`..`btn3`), bits 4..=7 are
//!   SW0..SW3 (`sw0`..`sw3`). The Pmod BTN has four buttons; the switches are
//!   on the same kind of module as the spec lists them (`VERIFY:`).
//! * `swt`: bits 0..=3 are SW0..SW3 (`sw0`..`sw3`); bits 4..=7 are driven low.
//!
//! Every pin the module drives is driven at its level (`PinDrive` mask 0xFF),
//! so an unused pin reads 0 at the PL, as on a pull-down. The PL sees a change
//! of the host's setting through the host-driven input path (`zs_pin_in`): the
//! engine sends the new levels with each `set_value`, so the testbench samples
//! them at its next tick.

use crate::{BoardDevice, DeviceImage, DeviceRegister, DeviceValue, PinDrive, readout};

/// Which module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    /// Pmod BTN: four buttons and four switches.
    Btn,
    /// Pmod SWT: four switches.
    Swt,
}

/// Buttons and switches as the host has set them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    module: Module,
    /// Buttons, bit n = BTN n (set = pressed).
    buttons: u8,
    /// Switches, bit n = SW n (set = closed).
    switches: u8,
}

impl Inputs {
    /// A Pmod BTN with everything released.
    pub fn btn() -> Inputs {
        Inputs {
            module: Module::Btn,
            buttons: 0,
            switches: 0,
        }
    }

    /// A Pmod SWT with every switch open.
    pub fn swt() -> Inputs {
        Inputs {
            module: Module::Swt,
            buttons: 0,
            switches: 0,
        }
    }

    /// The module.
    pub fn module(&self) -> Module {
        self.module
    }

    /// The buttons, bit n = BTN n.
    pub fn buttons(&self) -> u8 {
        self.buttons
    }

    /// The switches, bit n = SW n.
    pub fn switches(&self) -> u8 {
        self.switches
    }

    /// The eight pin levels the module drives.
    pub fn pin_levels(&self) -> u8 {
        match self.module {
            Module::Btn => self.buttons | self.switches << 4,
            Module::Swt => self.switches,
        }
    }

    fn kind_name(&self) -> &'static str {
        match self.module {
            Module::Btn => "btn",
            Module::Swt => "swt",
        }
    }
}

/// Maps a quantity name to its bit: `(is_button, index)`.
fn lookup(module: Module, name: &str) -> Option<(bool, u8)> {
    let lower = name.to_ascii_lowercase();
    let (is_btn, digits) = match lower.strip_prefix("btn") {
        Some(r) => (true, r),
        None => (false, lower.strip_prefix("sw")?),
    };
    if is_btn && module == Module::Swt {
        return None;
    }
    let n: u8 = digits.parse().ok().filter(|n| *n < 4)?;
    Some((is_btn, n))
}

impl BoardDevice for Inputs {
    fn kind(&self) -> &'static str {
        self.kind_name()
    }

    /// The PL's outputs do not reach these modules: nothing to record.
    fn pins(&mut self, _t_ns: u64, _levels: u8) {}

    fn drive(&self) -> PinDrive {
        PinDrive {
            mask: 0xFF,
            levels: self.pin_levels(),
        }
    }

    fn image(&self) -> Option<DeviceImage> {
        let bits = |v: u8| format!("{:04b}", v & 0x0F);
        Some(match self.module {
            Module::Btn => readout(&[
                "PMOD BTN",
                &format!("BTN {}", bits(self.buttons)),
                &format!("SW  {}", bits(self.switches)),
            ]),
            Module::Swt => readout(&["PMOD SWT", &format!("SW  {}", bits(self.switches))]),
        })
    }

    fn describe(&self) -> String {
        match self.module {
            Module::Btn => format!(
                "Pmod BTN: buttons {:04b}, switches {:04b}, pins {:#04x}",
                self.buttons,
                self.switches,
                self.pin_levels()
            ),
            Module::Swt => format!(
                "Pmod SWT: switches {:04b}, pins {:#04x}",
                self.switches,
                self.pin_levels()
            ),
        }
    }

    fn values(&self) -> Vec<DeviceValue> {
        let mut out = Vec::new();
        let names: &[&'static str] = match self.module {
            Module::Btn => &["btn0", "btn1", "btn2", "btn3", "sw0", "sw1", "sw2", "sw3"],
            Module::Swt => &["sw0", "sw1", "sw2", "sw3"],
        };
        for name in names {
            let (is_btn, n) = lookup(self.module, name).expect("listed names parse");
            let on = if is_btn {
                self.buttons >> n & 1
            } else {
                self.switches >> n & 1
            };
            out.push(DeviceValue {
                name,
                value: f64::from(on),
                unit: "0/1",
                min: 0.0,
                max: 1.0,
            });
        }
        out
    }

    fn set_value(&mut self, name: &str, value: f64) -> Result<(), String> {
        let (is_btn, n) = lookup(self.module, name).ok_or_else(|| {
            format!(
                "`{}` has no quantity `{name}`; available: {}",
                self.kind_name(),
                self.values()
                    .iter()
                    .map(|v| v.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let bit = 1u8 << n;
        let slot = if is_btn {
            &mut self.buttons
        } else {
            &mut self.switches
        };
        if value >= 0.5 {
            *slot |= bit;
        } else {
            *slot &= !bit;
        }
        Ok(())
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        vec![
            DeviceRegister {
                name: "BUTTONS".to_string(),
                addr: 0,
                value: u32::from(self.buttons),
            },
            DeviceRegister {
                name: "SWITCHES".to_string(),
                addr: 1,
                value: u32::from(self.switches),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_checked;

    #[test]
    fn the_pins_follow_the_host_set_levels() {
        let mut b = Inputs::btn();
        assert_eq!(
            b.drive(),
            PinDrive {
                mask: 0xFF,
                levels: 0
            }
        );
        set_checked(&mut b, "btn0", 1.0).unwrap();
        set_checked(&mut b, "btn3", 1.0).unwrap();
        set_checked(&mut b, "sw1", 1.0).unwrap();
        assert_eq!(b.buttons(), 0b1001);
        assert_eq!(b.switches(), 0b0010);
        assert_eq!(
            b.drive().levels,
            0b0010_1001,
            "BTN on pins 0..3, SW on 4..7"
        );
        assert_eq!(b.drive().mask, 0xFF, "every pin is driven");
    }

    #[test]
    fn swt_has_switches_only() {
        let mut s = Inputs::swt();
        assert_eq!(s.kind(), "swt");
        set_checked(&mut s, "sw3", 1.0).unwrap();
        assert_eq!(s.drive().levels, 0b1000, "SW3 on pin 3, the rest low");
        assert!(
            set_checked(&mut s, "btn0", 1.0).is_err(),
            "an SWT has no buttons"
        );
        assert_eq!(s.values().len(), 4);
    }

    #[test]
    fn values_are_set_by_name_and_read_back() {
        let mut b = Inputs::btn();
        assert_eq!(
            set_checked(&mut b, "BTN2", 0.9),
            Ok(0.9),
            "stored as pressed"
        );
        assert_eq!(b.buttons(), 0b0100);
        let v = b.values();
        assert_eq!(v.len(), 8);
        assert_eq!(v[2].name, "btn2");
        assert_eq!(v[2].value, 1.0);
        assert_eq!(v[0].value, 0.0);
        // Clamped to the quantity's range, then thresholded.
        assert_eq!(set_checked(&mut b, "btn2", 7.0), Ok(1.0));
        assert_eq!(set_checked(&mut b, "btn2", -3.0), Ok(0.0));
        assert_eq!(b.buttons(), 0);
        assert!(set_checked(&mut b, "btn4", 1.0).is_err());
        assert!(set_checked(&mut b, "btn0", f64::NAN).is_err());
    }

    #[test]
    fn the_host_setting_survives_pin_changes_from_the_pl() {
        let mut b = Inputs::btn();
        set_checked(&mut b, "sw0", 1.0).unwrap();
        b.pins(10, 0xFF);
        b.pins(20, 0x00);
        assert_eq!(b.switches(), 1, "the PL does not change the switches");
        assert_eq!(b.drive().levels, 0x10);
    }

    #[test]
    fn the_picture_is_a_text_card() {
        let img = Inputs::btn().image().unwrap();
        assert_eq!((img.width, img.height), (128, 32));
        assert!(img.rgba.chunks(4).any(|p| p[0] > 100), "text is drawn");
        assert!(Inputs::swt().image().is_some());
        assert!(Inputs::btn().describe().contains("pins 0x00"));
    }
}
