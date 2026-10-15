//! The Analog Devices ADT7420 temperature sensor on a Digilent Pmod TMP2
//! (I2C, address 0x4B with both address jumpers set).
//!
//! Pmod pin order of the eight levels: bit0 INT, bit1 CT, bit2 SCL, bit3 SDA.
//! `VERIFY:` the pin order against the Digilent reference manual.
//!
//! Modelled: the temperature register in 13-bit (0.0625 degC, flag bits in
//! the three LSBs zero) and 16-bit (1/128 degC) mode, status flags against
//! the three limits, configuration (resolution, operating mode: continuous
//! and 1 SPS follow the temperature, shutdown freezes it, one-shot converts
//! once on the configuration write and falls back to shutdown), the limit
//! registers with their reset values, the ID register and the software
//! reset. Not modelled: conversion time (a value is ready at once), the INT
//! and CT pins, fault queue, hysteresis in the flags.

use crate::i2c::{I2cSlave, PointerSlave, Registers};
use crate::{BoardDevice, DeviceImage, DeviceRegister, DeviceValue, PinDrive, readout};

/// I2C address of the Pmod TMP2 with its default jumpers.
pub const ADDRESS: u8 = 0x4B;
const SCL_BIT: u8 = 2;
const SDA_BIT: u8 = 3;

const REG_TEMP_MSB: u32 = 0x00;
const REG_TEMP_LSB: u32 = 0x01;
const REG_STATUS: u32 = 0x02;
const REG_CONFIG: u32 = 0x03;
const REG_THIGH: u32 = 0x04;
const REG_TLOW: u32 = 0x06;
const REG_TCRIT: u32 = 0x08;
const REG_THYST: u32 = 0x0A;
const REG_ID: u32 = 0x0B;
const REG_RESET: u32 = 0x2F;

const T_MIN: f64 = -55.0;
const T_MAX: f64 = 150.0;

/// The register file.
#[derive(Clone, Debug)]
struct Regs {
    /// The temperature the host set.
    physical: f64,
    /// The last converted temperature (what the registers show).
    sample: f64,
    config: u8,
    thigh: u16,
    tlow: u16,
    tcrit: u16,
    thyst: u8,
    /// The 16-bit value latched when the MSB was read.
    latched: u16,
    msb_read: bool,
    reads: u32,
    writes: u32,
}

impl Regs {
    fn new() -> Regs {
        Regs {
            physical: 25.0,
            sample: 25.0,
            config: 0,
            thigh: 0x2000,
            tlow: 0x0500,
            tcrit: 0x4980,
            thyst: 0x05,
            latched: 0,
            msb_read: false,
            reads: 0,
            writes: 0,
        }
    }

    fn sixteen_bit(&self) -> bool {
        self.config & 0x80 != 0
    }

    /// Encodes `c` as the 16-bit register content in the current resolution.
    fn encode(&self, c: f64) -> u16 {
        if self.sixteen_bit() {
            (c * 128.0).round().clamp(-32768.0, 32767.0) as i16 as u16
        } else {
            let raw = (c / 0.0625).round().clamp(-4096.0, 4095.0) as i16;
            (raw as u16) << 3
        }
    }

    /// The temperature a 16-bit register content stands for.
    fn decode(&self, raw: u16) -> f64 {
        if self.sixteen_bit() {
            f64::from(raw as i16) / 128.0
        } else {
            f64::from(raw as i16 >> 3) * 0.0625
        }
    }

    fn status(&self) -> u8 {
        let t = self.sample;
        let mut s = 0u8; // bit 7 RDY = 0: a result is always ready
        if t >= self.decode(self.tcrit) {
            s |= 1 << 2;
        }
        if t > self.decode(self.thigh) {
            s |= 1 << 3;
        }
        if t < self.decode(self.tlow) {
            s |= 1 << 4;
        }
        s
    }

    /// Converts when the operating mode lets the part measure.
    fn follow(&mut self) {
        if self.config >> 5 & 3 == 0 || self.config >> 5 & 3 == 2 {
            self.sample = self.physical;
        }
    }
}

impl Registers for Regs {
    fn read(&mut self, reg: u32) -> u8 {
        self.reads += 1;
        let pair = |v: u16, lsb: bool| if lsb { v as u8 } else { (v >> 8) as u8 };
        match reg {
            REG_TEMP_MSB => {
                self.latched = self.encode(self.sample);
                self.msb_read = true;
                (self.latched >> 8) as u8
            }
            REG_TEMP_LSB => {
                let v = if self.msb_read {
                    self.latched
                } else {
                    self.encode(self.sample)
                };
                self.msb_read = false;
                v as u8
            }
            REG_STATUS => self.status(),
            REG_CONFIG => self.config,
            r if r == REG_THIGH || r == REG_THIGH + 1 => pair(self.thigh, r & 1 != 0),
            r if r == REG_TLOW || r == REG_TLOW + 1 => pair(self.tlow, r & 1 != 0),
            r if r == REG_TCRIT || r == REG_TCRIT + 1 => pair(self.tcrit, r & 1 != 0),
            REG_THYST => self.thyst,
            REG_ID => 0xCB,
            _ => 0,
        }
    }

    fn write(&mut self, reg: u32, v: u8) {
        self.writes += 1;
        let set = |word: &mut u16, lsb: bool| {
            *word = if lsb {
                *word & 0xFF00 | u16::from(v)
            } else {
                *word & 0x00FF | u16::from(v) << 8
            };
        };
        match reg {
            REG_CONFIG => {
                self.config = v;
                if v >> 5 & 3 == 1 {
                    // One-shot: convert once, then shut down.
                    self.sample = self.physical;
                    self.config = v | 0x60;
                } else {
                    self.follow();
                }
            }
            r if r == REG_THIGH || r == REG_THIGH + 1 => set(&mut self.thigh, r & 1 != 0),
            r if r == REG_TLOW || r == REG_TLOW + 1 => set(&mut self.tlow, r & 1 != 0),
            r if r == REG_TCRIT || r == REG_TCRIT + 1 => set(&mut self.tcrit, r & 1 != 0),
            REG_THYST => self.thyst = v & 0x0F,
            REG_RESET => {
                let (physical, reads, writes) = (self.physical, self.reads, self.writes);
                *self = Regs::new();
                self.physical = physical;
                self.sample = physical;
                self.reads = reads;
                self.writes = writes;
            }
            _ => {}
        }
    }
}

/// The ADT7420 behind a Pmod TMP2.
#[derive(Clone, Debug)]
pub struct Adt7420 {
    bus: I2cSlave,
    dev: PointerSlave<Regs>,
}

impl Default for Adt7420 {
    fn default() -> Adt7420 {
        Adt7420::new()
    }
}

impl Adt7420 {
    /// A sensor at 25 degC in its power-on state.
    pub fn new() -> Adt7420 {
        Adt7420 {
            bus: I2cSlave::new(SCL_BIT, SDA_BIT),
            dev: PointerSlave::new(ADDRESS, 1, Regs::new()),
        }
    }

    /// The temperature the host set, in degrees Celsius.
    pub fn temperature(&self) -> f64 {
        self.dev.regs.physical
    }

    /// The 16-bit content of the temperature register as the part would show
    /// it now (resolution per the configuration register).
    pub fn temperature_register(&self) -> u16 {
        self.dev.regs.encode(self.dev.regs.sample)
    }
}

impl BoardDevice for Adt7420 {
    fn kind(&self) -> &'static str {
        "tmp2"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        self.bus.pins(&mut self.dev, levels);
    }

    fn drive(&self) -> PinDrive {
        PinDrive {
            mask: 1 << SDA_BIT,
            levels: u8::from(self.bus.sda_released()) << SDA_BIT,
        }
    }

    fn image(&self) -> Option<DeviceImage> {
        let r = &self.dev.regs;
        Some(readout(&[
            "TMP2 ADT7420",
            &format!("{:.2} C", r.sample),
            &format!(
                "{} BIT  I2C {ADDRESS:#04X}",
                if r.sixteen_bit() { 16 } else { 13 }
            ),
            &format!("{} READS", r.reads),
        ]))
    }

    fn describe(&self) -> String {
        let r = &self.dev.regs;
        format!(
            "ADT7420 (Pmod TMP2) I2C {ADDRESS:#04x}, {:.4} degC set, register {:#06x} ({}-bit), \
             config {:#04x}, {} register read(s), {} write(s)",
            r.physical,
            self.temperature_register(),
            if r.sixteen_bit() { 16 } else { 13 },
            r.config,
            r.reads,
            r.writes,
        )
    }

    fn values(&self) -> Vec<DeviceValue> {
        vec![DeviceValue {
            name: "temperature",
            value: self.dev.regs.physical,
            unit: "degC",
            min: T_MIN,
            max: T_MAX,
        }]
    }

    fn set_value(&mut self, name: &str, value: f64) -> Result<(), String> {
        if !name.eq_ignore_ascii_case("temperature") {
            return Err(format!(
                "`tmp2` has no quantity `{name}`; available: temperature"
            ));
        }
        let r = &mut self.dev.regs;
        r.physical = value.clamp(T_MIN, T_MAX);
        r.follow();
        Ok(())
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        let r = &self.dev.regs;
        let reg = |name: &str, addr: u32, value: u32| DeviceRegister {
            name: name.to_string(),
            addr,
            value,
        };
        vec![
            reg("TEMP", 0x00, u32::from(self.temperature_register())),
            reg("STATUS", 0x02, u32::from(r.status())),
            reg("CONFIG", 0x03, u32::from(r.config)),
            reg("THIGH", 0x04, u32::from(r.thigh)),
            reg("TLOW", 0x06, u32::from(r.tlow)),
            reg("TCRIT", 0x08, u32::from(r.tcrit)),
            reg("THYST", 0x0A, u32::from(r.thyst)),
            reg("ID", 0x0B, 0xCB),
            reg("POINTER", 0xFF, self.dev.pointer()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::tests::Master;
    use crate::{I2cDevice, set_checked};

    /// Reads `n` registers starting at `reg` with a repeated START.
    fn read(m: &mut Master<PointerSlave<Regs>>, reg: u8, n: usize) -> Vec<u8> {
        m.start();
        assert!(m.write_byte(ADDRESS << 1));
        assert!(m.write_byte(reg));
        m.start();
        assert!(m.write_byte(ADDRESS << 1 | 1));
        let v = (0..n).map(|i| m.read_byte(i + 1 < n)).collect();
        m.stop();
        v
    }

    fn master() -> Master<PointerSlave<Regs>> {
        Master::new(PointerSlave::new(ADDRESS, 1, Regs::new()))
    }

    fn word(v: &[u8]) -> u16 {
        u16::from(v[0]) << 8 | u16::from(v[1])
    }

    #[test]
    fn identifies_itself() {
        let mut m = master();
        assert_eq!(read(&mut m, 0x0B, 1), vec![0xCB]);
        assert_eq!(m.dev.address(), 0x4B);
    }

    #[test]
    fn thirteen_bit_temperature_is_in_sixteenths() {
        let mut m = master();
        m.dev.regs.physical = 23.5;
        m.dev.regs.follow();
        let raw = word(&read(&mut m, 0x00, 2));
        assert_eq!(raw >> 3, 376, "23.5 / 0.0625");
        assert_eq!(raw & 7, 0);
        m.dev.regs.physical = -12.25;
        m.dev.regs.follow();
        let raw = word(&read(&mut m, 0x00, 2));
        assert_eq!(raw as i16 >> 3, -196, "two's complement");
    }

    #[test]
    fn sixteen_bit_mode_is_in_128ths() {
        let mut m = master();
        m.write(ADDRESS, &[0x03, 0x80]);
        assert_eq!(read(&mut m, 0x03, 1), vec![0x80]);
        m.dev.regs.physical = 23.5;
        m.dev.regs.follow();
        assert_eq!(word(&read(&mut m, 0x00, 2)), 3008, "23.5 * 128");
        m.dev.regs.physical = -0.5;
        m.dev.regs.follow();
        assert_eq!(word(&read(&mut m, 0x00, 2)) as i16, -64);
    }

    #[test]
    fn status_flags_follow_the_limits() {
        let mut m = master();
        m.dev.regs.physical = 70.0; // above THIGH (64), below TCRIT (147)
        m.dev.regs.follow();
        assert_eq!(read(&mut m, 0x02, 1)[0] & 0x1C, 1 << 3);
        m.dev.regs.physical = 5.0; // below TLOW (10)
        m.dev.regs.follow();
        assert_eq!(read(&mut m, 0x02, 1)[0] & 0x1C, 1 << 4);
        m.dev.regs.physical = 148.0;
        m.dev.regs.follow();
        assert_eq!(read(&mut m, 0x02, 1)[0] & 0x1C, 1 << 3 | 1 << 2);
        m.dev.regs.physical = 25.0;
        m.dev.regs.follow();
        assert_eq!(read(&mut m, 0x02, 1)[0], 0);
    }

    #[test]
    fn limits_have_reset_values_and_are_writable() {
        let mut m = master();
        assert_eq!(word(&read(&mut m, 0x04, 2)), 0x2000);
        assert_eq!(word(&read(&mut m, 0x08, 2)), 0x4980);
        m.write(ADDRESS, &[0x04, 0x12, 0x34]);
        assert_eq!(word(&read(&mut m, 0x04, 2)), 0x1234);
        assert_eq!(read(&mut m, 0x0A, 1), vec![0x05]);
    }

    #[test]
    fn shutdown_freezes_and_one_shot_converts_once() {
        let mut m = master();
        m.dev.regs.physical = 30.0;
        m.dev.regs.follow();
        m.write(ADDRESS, &[0x03, 0x60]); // shutdown
        m.dev.regs.physical = 40.0;
        m.dev.regs.follow();
        assert_eq!(word(&read(&mut m, 0x00, 2)) >> 3, 480, "still 30 degC");
        m.write(ADDRESS, &[0x03, 0x20]); // one-shot
        assert_eq!(word(&read(&mut m, 0x00, 2)) >> 3, 640, "now 40 degC");
        assert_eq!(read(&mut m, 0x03, 1)[0] >> 5 & 3, 3, "back to shutdown");
    }

    #[test]
    fn software_reset_restores_defaults_but_not_the_temperature() {
        let mut m = master();
        m.dev.regs.physical = 33.0;
        m.write(ADDRESS, &[0x03, 0x80]);
        m.write(ADDRESS, &[0x2F, 0x00]);
        assert_eq!(read(&mut m, 0x03, 1), vec![0x00]);
        assert_eq!(word(&read(&mut m, 0x00, 2)) >> 3, 528);
    }

    #[test]
    fn the_msb_read_latches_the_lsb() {
        let mut m = master();
        m.dev.regs.physical = 23.5;
        m.dev.regs.follow();
        m.start();
        assert!(m.write_byte(ADDRESS << 1));
        assert!(m.write_byte(0x00));
        m.start();
        assert!(m.write_byte(ADDRESS << 1 | 1));
        let msb = m.read_byte(true);
        // The host changes the temperature between the two bytes.
        m.dev.regs.physical = 80.0;
        m.dev.regs.follow();
        let lsb = m.read_byte(false);
        m.stop();
        assert_eq!(u16::from(msb) << 8 | u16::from(lsb), 376 << 3);
    }

    #[test]
    fn the_board_device_wraps_the_bus_and_values() {
        let mut d = Adt7420::new();
        assert_eq!(d.kind(), "tmp2");
        assert_eq!(d.drive().mask, 1 << 3);
        assert_eq!(set_checked(&mut d, "Temperature", 23.5), Ok(23.5));
        assert_eq!(set_checked(&mut d, "temperature", 500.0), Ok(150.0));
        assert!(set_checked(&mut d, "humidity", 1.0).is_err());
        assert!(set_checked(&mut d, "temperature", f64::NAN).is_err());
        assert!((d.temperature() - 150.0).abs() < 1e-9);
        assert_eq!(d.values()[0].name, "temperature");
        assert!(d.describe().contains("0x4b"));
        let img = d.image().unwrap();
        assert_eq!((img.width, img.height), (128, 32));
        let regs = d.registers();
        assert!(regs.iter().any(|r| r.name == "ID" && r.value == 0xCB));
    }
}
