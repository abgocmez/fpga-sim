//! The Analog Devices ADXL345 accelerometer on a Digilent Pmod ACL (SPI,
//! 4-wire, mode 3).
//!
//! Pmod pin order of the eight levels: bit0 CS (active low), bit1 SDI (MOSI),
//! bit2 SDO (MISO), bit3 SCLK, bit4 INT1, bit5 INT2. `VERIFY:` the pin order
//! against the Digilent reference manual.
//!
//! Transfers: the first byte is `R/W` (1 = read), `MB` (multi-byte, the
//! address auto-increments) and a 6-bit register address; data bytes follow.
//!
//! Modelled: the register file (read-only DEVID, data, INT_SOURCE,
//! ACT_TAP_STATUS, FIFO_STATUS), `DATA_FORMAT` range, full-resolution and
//! justify bits, the three offset registers (15.6 mg/LSB), `POWER_CTL`
//! measure bit (the data registers are zero until measurement is switched on
//! and freeze when it is switched off), `BW_RATE`, and the INT1/INT2 pins for
//! the data-ready interrupt (`INT_ENABLE`, `INT_MAP`, `INT_INVERT`; they are
//! updated whenever a pin event reaches the model). Not modelled: the FIFO,
//! tap, activity and free-fall detection, self test, sleep mode, the 3-wire
//! SPI mode (`DATA_FORMAT.SPI`) and the I2C interface.

use crate::spi::{SpiDevice, SpiSlave};
use crate::{BoardDevice, DeviceImage, DeviceRegister, DeviceValue, PinDrive, readout};

const CS_BIT: u8 = 0;
const MOSI_BIT: u8 = 1;
const MISO_BIT: u8 = 2;
const SCLK_BIT: u8 = 3;
const INT1_BIT: u8 = 4;
const INT2_BIT: u8 = 5;

const DEVID: u8 = 0x00;
const OFSX: u8 = 0x1E;
const BW_RATE: u8 = 0x2C;
const POWER_CTL: u8 = 0x2D;
const INT_ENABLE: u8 = 0x2E;
const INT_MAP: u8 = 0x2F;
const INT_SOURCE: u8 = 0x30;
const DATA_FORMAT: u8 = 0x31;
const DATAX0: u8 = 0x32;
const FIFO_STATUS: u8 = 0x39;

const DATA_READY: u8 = 0x80;

#[derive(Clone, Debug)]
struct Chip {
    regs: [u8; 0x3A],
    /// What the host set, in g.
    accel: [f64; 3],
    /// Data registers as of the last time measurement was on.
    data: [u8; 6],
    /// Transfer state: the command byte and the address it points at.
    cmd: Option<(bool, bool, u8)>,
    wrote_once: bool,
    reads: u32,
    writes: u32,
}

impl Chip {
    fn new() -> Chip {
        let mut regs = [0u8; 0x3A];
        regs[usize::from(DEVID)] = 0xE5;
        regs[usize::from(BW_RATE)] = 0x0A;
        Chip {
            regs,
            accel: [0.0, 0.0, 1.0],
            data: [0; 6],
            cmd: None,
            wrote_once: false,
            reads: 0,
            writes: 0,
        }
    }

    fn measuring(&self) -> bool {
        self.regs[usize::from(POWER_CTL)] & 0x08 != 0
    }

    /// The six data registers for the current physical input and format.
    fn sample(&self) -> [u8; 6] {
        let df = self.regs[usize::from(DATA_FORMAT)];
        let range = df & 3;
        let full = df & 0x08 != 0;
        let left = df & 0x04 != 0;
        let (lsb_per_g, bits) = if full {
            (256.0, 10 + u32::from(range))
        } else {
            (
                match range {
                    0 => 256.0,
                    1 => 128.0,
                    2 => 64.0,
                    _ => 32.0,
                },
                10,
            )
        };
        let half = f64::from(1u32 << (bits - 1));
        let mut out = [0u8; 6];
        for axis in 0..3 {
            let ofs = f64::from(self.regs[usize::from(OFSX) + axis] as i8);
            let counts = (self.accel[axis] * lsb_per_g + ofs * 0.0156 * lsb_per_g)
                .round()
                .clamp(-half, half - 1.0) as i16;
            let word = if left {
                (counts << (16 - bits)) as u16
            } else {
                counts as u16
            };
            out[axis * 2] = word as u8;
            out[axis * 2 + 1] = (word >> 8) as u8;
        }
        out
    }

    fn data_ready(&self) -> bool {
        self.measuring()
    }

    fn int_source(&self) -> u8 {
        0x02 | if self.data_ready() { DATA_READY } else { 0 }
    }

    fn read_reg(&mut self, addr: u8) -> u8 {
        self.reads += 1;
        match addr {
            DATAX0..=0x37 => {
                if self.measuring() {
                    self.data = self.sample();
                }
                self.data[usize::from(addr - DATAX0)]
            }
            INT_SOURCE => self.int_source(),
            0x00..=FIFO_STATUS => self.regs[usize::from(addr)],
            _ => 0,
        }
    }

    fn write_reg(&mut self, addr: u8, v: u8) {
        self.writes += 1;
        let read_only = matches!(
            addr,
            DEVID | 0x2B | INT_SOURCE | DATAX0..=0x37 | FIFO_STATUS
        );
        if read_only || usize::from(addr) >= self.regs.len() {
            return;
        }
        self.regs[usize::from(addr)] = v;
        if addr == POWER_CTL && v & 0x08 != 0 {
            self.data = self.sample();
        }
    }

    /// The INT1 and INT2 pin levels.
    fn int_pins(&self) -> (bool, bool) {
        let active = self.int_source() & self.regs[usize::from(INT_ENABLE)];
        let map = self.regs[usize::from(INT_MAP)];
        let invert = self.regs[usize::from(DATA_FORMAT)] & 0x20 != 0;
        let int1 = active & !map != 0;
        let int2 = active & map != 0;
        (int1 != invert, int2 != invert)
    }
}

impl SpiDevice for Chip {
    fn select(&mut self) -> u8 {
        self.cmd = None;
        self.wrote_once = false;
        0
    }

    fn exchange(&mut self, mosi: u8) -> u8 {
        match self.cmd {
            None => {
                let read = mosi & 0x80 != 0;
                let multi = mosi & 0x40 != 0;
                let addr = mosi & 0x3F;
                self.cmd = Some((read, multi, addr));
                if read {
                    let v = self.read_reg(addr);
                    self.cmd = Some((true, multi, addr.wrapping_add(1) & 0x3F));
                    v
                } else {
                    0
                }
            }
            Some((true, multi, addr)) => {
                if multi {
                    let v = self.read_reg(addr);
                    self.cmd = Some((true, true, addr.wrapping_add(1) & 0x3F));
                    v
                } else {
                    0
                }
            }
            Some((false, multi, addr)) => {
                if multi || !self.wrote_once {
                    self.write_reg(addr, mosi);
                    self.wrote_once = true;
                    self.cmd = Some((false, multi, addr.wrapping_add(1) & 0x3F));
                }
                0
            }
        }
    }

    fn deselect(&mut self) {
        self.cmd = None;
    }
}

/// The ADXL345 on a Pmod ACL.
#[derive(Clone, Debug)]
pub struct Adxl345 {
    spi: SpiSlave,
    chip: Chip,
}

impl Default for Adxl345 {
    fn default() -> Adxl345 {
        Adxl345::new()
    }
}

impl Adxl345 {
    /// A sensor in standby, lying flat (0 g, 0 g, +1 g).
    pub fn new() -> Adxl345 {
        Adxl345 {
            spi: SpiSlave::new(CS_BIT, MOSI_BIT, MISO_BIT, SCLK_BIT, 3),
            chip: Chip::new(),
        }
    }

    /// The acceleration the host set, in g.
    pub fn acceleration(&self) -> [f64; 3] {
        self.chip.accel
    }
}

const AXES: [&str; 3] = ["accel_x", "accel_y", "accel_z"];

impl BoardDevice for Adxl345 {
    fn kind(&self) -> &'static str {
        "acl"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        self.spi.pins(&mut self.chip, levels);
    }

    fn drive(&self) -> PinDrive {
        let (i1, i2) = self.chip.int_pins();
        let miso = self.spi.drive();
        PinDrive {
            mask: miso.mask | 1 << INT1_BIT | 1 << INT2_BIT,
            levels: miso.levels | u8::from(i1) << INT1_BIT | u8::from(i2) << INT2_BIT,
        }
    }

    fn image(&self) -> Option<DeviceImage> {
        let a = self.chip.accel;
        let state = if self.chip.measuring() {
            "MEASURING"
        } else {
            "STANDBY"
        };
        Some(readout(&[
            "ACL ADXL345",
            &format!("X {:.3} G", a[0]),
            &format!("Y {:.3} G   {state}", a[1]),
            &format!("Z {:.3} G", a[2]),
        ]))
    }

    fn describe(&self) -> String {
        let a = self.chip.accel;
        let d = self.chip.sample();
        let axis = |i: usize| i16::from_le_bytes([d[i * 2], d[i * 2 + 1]]);
        format!(
            "ADXL345 (Pmod ACL) SPI mode 3, {} g set ({:.3}, {:.3}, {:.3}), output {} {} {}, \
             {}, {} register read(s), {} write(s)",
            "xyz",
            a[0],
            a[1],
            a[2],
            axis(0),
            axis(1),
            axis(2),
            if self.chip.measuring() {
                "measuring"
            } else {
                "standby"
            },
            self.chip.reads,
            self.chip.writes,
        )
    }

    fn values(&self) -> Vec<DeviceValue> {
        AXES.iter()
            .zip(self.chip.accel)
            .map(|(name, value)| DeviceValue {
                name,
                value,
                unit: "g",
                min: -16.0,
                max: 16.0,
            })
            .collect()
    }

    fn set_value(&mut self, name: &str, value: f64) -> Result<(), String> {
        let i = AXES
            .iter()
            .position(|a| a.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                format!(
                    "`acl` has no quantity `{name}`; available: {}",
                    AXES.join(", ")
                )
            })?;
        self.chip.accel[i] = value.clamp(-16.0, 16.0);
        Ok(())
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        let reg = |name: &str, addr: u8, value: u8| DeviceRegister {
            name: name.to_string(),
            addr: u32::from(addr),
            value: u32::from(value),
        };
        let c = &self.chip;
        let mut v = vec![
            reg("DEVID", DEVID, c.regs[0]),
            reg("OFSX", 0x1E, c.regs[0x1E]),
            reg("OFSY", 0x1F, c.regs[0x1F]),
            reg("OFSZ", 0x20, c.regs[0x20]),
            reg("BW_RATE", BW_RATE, c.regs[usize::from(BW_RATE)]),
            reg("POWER_CTL", POWER_CTL, c.regs[usize::from(POWER_CTL)]),
            reg("INT_ENABLE", INT_ENABLE, c.regs[usize::from(INT_ENABLE)]),
            reg("INT_MAP", INT_MAP, c.regs[usize::from(INT_MAP)]),
            reg("INT_SOURCE", INT_SOURCE, c.int_source()),
            reg("DATA_FORMAT", DATA_FORMAT, c.regs[usize::from(DATA_FORMAT)]),
        ];
        let d = if c.measuring() { c.sample() } else { c.data };
        for (i, name) in ["DATAX0", "DATAX1", "DATAY0", "DATAY1", "DATAZ0", "DATAZ1"]
            .iter()
            .enumerate()
        {
            v.push(reg(name, DATAX0 + i as u8, d[i]));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_checked;
    use crate::spi::tests::Master;

    fn master() -> Master<Chip> {
        Master::new(Chip::new(), 3)
    }

    fn read(m: &mut Master<Chip>, addr: u8, n: usize) -> Vec<u8> {
        m.select();
        m.byte(0x80 | if n > 1 { 0x40 } else { 0 } | addr);
        let v = (0..n).map(|_| m.byte(0)).collect();
        m.deselect();
        v
    }

    fn write(m: &mut Master<Chip>, addr: u8, data: &[u8]) {
        m.select();
        m.byte(if data.len() > 1 { 0x40 } else { 0 } | addr);
        for &b in data {
            m.byte(b);
        }
        m.deselect();
    }

    fn axis(d: &[u8], i: usize) -> i16 {
        i16::from_le_bytes([d[i * 2], d[i * 2 + 1]])
    }

    #[test]
    fn devid_and_reset_values() {
        let mut m = master();
        assert_eq!(read(&mut m, 0x00, 1), vec![0xE5]);
        assert_eq!(read(&mut m, 0x2C, 1), vec![0x0A], "BW_RATE");
        assert_eq!(read(&mut m, 0x2D, 1), vec![0x00], "POWER_CTL");
        assert_eq!(read(&mut m, 0x31, 1), vec![0x00], "DATA_FORMAT");
    }

    #[test]
    fn data_is_zero_in_standby_and_live_while_measuring() {
        let mut m = master();
        m.dev.accel = [0.5, -0.25, 1.0];
        assert_eq!(read(&mut m, 0x32, 6), vec![0; 6], "standby");
        write(&mut m, 0x2D, &[0x08]);
        let d = read(&mut m, 0x32, 6);
        // +-2 g, 10 bit: 256 LSB/g.
        assert_eq!([axis(&d, 0), axis(&d, 1), axis(&d, 2)], [128, -64, 256]);
        m.dev.accel = [0.0, 0.0, 0.0];
        let d = read(&mut m, 0x32, 6);
        assert_eq!(d, vec![0; 6]);
        write(&mut m, 0x2D, &[0x00]);
        m.dev.accel = [1.0, 1.0, 1.0];
        assert_eq!(
            read(&mut m, 0x32, 6),
            vec![0; 6],
            "frozen at the last value"
        );
    }

    #[test]
    fn range_and_full_resolution_scale_the_output() {
        let mut m = master();
        m.dev.accel = [1.0, 0.0, -1.0];
        write(&mut m, 0x2D, &[0x08]);
        for (df, want) in [
            (0x00u8, 256i16), // +-2 g, 10 bit
            (0x01, 128),      // +-4 g
            (0x02, 64),       // +-8 g
            (0x03, 32),       // +-16 g
            (0x0B, 256),      // full resolution at +-16 g is still 256 LSB/g
        ] {
            write(&mut m, 0x31, &[df]);
            let d = read(&mut m, 0x32, 6);
            assert_eq!(axis(&d, 0), want, "DATA_FORMAT {df:#04x}");
            assert_eq!(axis(&d, 2), -want);
        }
    }

    #[test]
    fn output_saturates_at_the_width_of_the_format() {
        let mut m = master();
        m.dev.accel = [16.0, -16.0, 0.0];
        write(&mut m, 0x2D, &[0x08]);
        let d = read(&mut m, 0x32, 6);
        assert_eq!([axis(&d, 0), axis(&d, 1)], [511, -512], "10 bit");
        write(&mut m, 0x31, &[0x0B]);
        let d = read(&mut m, 0x32, 6);
        assert_eq!([axis(&d, 0), axis(&d, 1)], [4095, -4096], "13 bit");
    }

    #[test]
    fn justify_left_aligns_the_value() {
        let mut m = master();
        m.dev.accel = [1.0, 0.0, 0.0];
        write(&mut m, 0x2D, &[0x08]);
        write(&mut m, 0x31, &[0x04]);
        let d = read(&mut m, 0x32, 2);
        assert_eq!(axis(&d, 0), 256 << 6);
    }

    #[test]
    fn offsets_are_added_in_15_6_mg_steps() {
        let mut m = master();
        m.dev.accel = [0.0, 0.0, 1.0];
        write(&mut m, 0x2D, &[0x08]);
        write(&mut m, 0x1E, &[0xFE, 0x03, 0xF0]);
        let d = read(&mut m, 0x32, 6);
        assert_eq!(axis(&d, 0), -8, "-2 * 15.6 mg = -4 LSB... of 3.9 mg: -8");
        assert_eq!(axis(&d, 1), 12);
        assert_eq!(axis(&d, 2), 256 - 64);
    }

    #[test]
    fn read_only_registers_ignore_writes_and_multi_byte_writes_increment() {
        let mut m = master();
        write(&mut m, 0x00, &[0x12]);
        assert_eq!(read(&mut m, 0x00, 1), vec![0xE5]);
        write(&mut m, 0x1D, &[1, 2, 3, 4]);
        assert_eq!(read(&mut m, 0x1D, 4), vec![1, 2, 3, 4]);
        // Single-byte write: only the first data byte is taken.
        m.select();
        m.byte(0x24);
        m.byte(0x55);
        m.byte(0x66);
        m.deselect();
        assert_eq!(read(&mut m, 0x24, 2), vec![0x55, 0x00]);
    }

    #[test]
    fn data_ready_drives_int1_and_follows_the_mapping() {
        let mut d = Adxl345::new();
        let int1 = 1u8 << INT1_BIT;
        let int2 = 1u8 << INT2_BIT;
        assert_eq!(d.drive().levels & (int1 | int2), 0);
        d.chip.regs[usize::from(INT_ENABLE)] = DATA_READY;
        d.chip.regs[usize::from(POWER_CTL)] = 0x08;
        assert_eq!(d.drive().levels & (int1 | int2), int1);
        d.chip.regs[usize::from(INT_MAP)] = DATA_READY;
        assert_eq!(d.drive().levels & (int1 | int2), int2);
        d.chip.regs[usize::from(DATA_FORMAT)] = 0x20; // active low
        assert_eq!(d.drive().levels & (int1 | int2), int1);
    }

    #[test]
    fn the_board_device_sets_axes_by_name() {
        let mut d = Adxl345::new();
        assert_eq!(d.kind(), "acl");
        assert_eq!(set_checked(&mut d, "accel_x", 0.5), Ok(0.5));
        assert_eq!(set_checked(&mut d, "ACCEL_Z", 99.0), Ok(16.0));
        assert!(set_checked(&mut d, "accel_w", 1.0).is_err());
        assert_eq!(d.acceleration(), [0.5, 0.0, 16.0]);
        assert_eq!(d.values().len(), 3);
        assert!(d.image().is_some());
        assert!(d.registers().iter().any(|r| r.name == "DEVID"));
    }
}
