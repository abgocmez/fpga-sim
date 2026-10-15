//! The devices plugged into a board's Pmod ports.

use crate::BoardDevice;

/// Devices by Pmod port (0..=4 are JA..JE), at most one per port.
#[derive(Default)]
pub struct Bank {
    devs: Vec<(u8, Box<dyn BoardDevice>)>,
}

impl Bank {
    /// An empty bank.
    pub fn new() -> Bank {
        Bank::default()
    }

    /// Plugs `dev` into `port`. Fails when the port is taken.
    pub fn attach(&mut self, port: u8, dev: Box<dyn BoardDevice>) -> Result<(), String> {
        if self.devs.iter().any(|(p, _)| *p == port) {
            return Err(format!("port {port} already has a device"));
        }
        self.devs.push((port, dev));
        Ok(())
    }

    /// The pin levels of `group` changed to `levels` at `t_ns`. Delivers them
    /// to the device on that port and returns the levels the board drives
    /// into the PL's input pins of the group: every pin a device drives at
    /// its level, every other pin released (1).
    pub fn pins(&mut self, group: u8, t_ns: u64, levels: u8) -> u8 {
        let mut input = 0xFFu8;
        for (port, dev) in &mut self.devs {
            if *port == group {
                dev.pins(t_ns, levels);
                let d = dev.drive();
                // A pin the device drives is pulled to its level; the wired
                // AND with other drivers is the AND of the contributions.
                input &= d.levels | !d.mask;
            }
        }
        input
    }

    /// The levels the devices on `group` drive now, without a pin change.
    pub fn input_levels(&self, group: u8) -> u8 {
        let mut input = 0xFFu8;
        for (port, dev) in &self.devs {
            if *port == group {
                let d = dev.drive();
                input &= d.levels | !d.mask;
            }
        }
        input
    }

    /// The device on `port`.
    pub fn get(&self, port: u8) -> Option<&dyn BoardDevice> {
        self.devs
            .iter()
            .find(|(p, _)| *p == port)
            .map(|(_, d)| d.as_ref())
    }

    /// The device on `port`, mutably.
    pub fn get_mut(&mut self, port: u8) -> Option<&mut dyn BoardDevice> {
        for (p, d) in &mut self.devs {
            if *p == port {
                return Some(d.as_mut());
            }
        }
        None
    }

    /// `(port, device)` in attachment order.
    pub fn iter(&self) -> impl Iterator<Item = (u8, &dyn BoardDevice)> {
        self.devs.iter().map(|(p, d)| (*p, d.as_ref()))
    }

    /// Number of devices.
    pub fn len(&self) -> usize {
        self.devs.len()
    }

    /// True when no device is attached.
    pub fn is_empty(&self) -> bool {
        self.devs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PinDrive, create};

    /// Drives pin 3 low while pin 0 of the port is low.
    struct Echo(u8);

    impl BoardDevice for Echo {
        fn kind(&self) -> &'static str {
            "echo"
        }
        fn pins(&mut self, _t: u64, levels: u8) {
            self.0 = levels;
        }
        fn drive(&self) -> PinDrive {
            PinDrive {
                mask: 1 << 3,
                levels: (self.0 & 1) << 3,
            }
        }
        fn describe(&self) -> String {
            "echo".to_string()
        }
    }

    #[test]
    fn undriven_pins_are_released_and_driven_ones_follow_the_device() {
        let mut b = Bank::new();
        assert_eq!(b.pins(0, 0, 0x00), 0xFF, "no device: everything released");
        b.attach(0, Box::new(Echo(0))).unwrap();
        assert_eq!(b.pins(0, 1, 0x00), 0xF7, "pin 3 pulled low");
        assert_eq!(b.pins(0, 2, 0x01), 0xFF);
        assert_eq!(b.pins(1, 3, 0x00), 0xFF, "another port is not affected");
        assert_eq!(b.input_levels(0), 0xFF);
    }

    #[test]
    fn one_device_per_port_and_lookup() {
        let mut b = Bank::new();
        assert!(b.is_empty());
        b.attach(2, create("oled").unwrap()).unwrap();
        assert!(b.attach(2, create("tmp2").unwrap()).is_err());
        b.attach(3, create("tmp2").unwrap()).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b.get(3).unwrap().kind(), "tmp2");
        assert!(b.get(0).is_none());
        assert!(b.get_mut(2).is_some());
        let ports: Vec<u8> = b.iter().map(|(p, _)| p).collect();
        assert_eq!(ports, vec![2, 3]);
    }
}
