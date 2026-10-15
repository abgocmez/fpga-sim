//! Models of hardware that plugs into the board's pins.
//!
//! A device sees the levels of a Pmod port (eight signal pins, bit *n* = pin
//! *n*, in Pmod order 1, 2, 3, 4, 7, 8, 9, 10) with the simulated time of every
//! change, and may show a picture. Devices are pure Rust and independent of
//! the PL and PS backends: they consume pin events and, for modules that talk
//! back (an I2C slave's ACK, an SPI slave's MISO), say which pins they drive.
//!
//! The models follow the datasheets as remembered by their author and have
//! **not** been compared with the hardware (`VERIFY:` in `STATUS.md`).

mod adc081;
mod adt7420;
mod adxl345;
mod bank;
mod i2c;
mod ov5640;
mod readout;
mod spi;
mod ssd1306;

pub mod font5x7 {
    //! The 5x7 font shared with the firmware examples.
    include!("font5x7.rs");
}

pub use adc081::Adc081;
pub use adt7420::Adt7420;
pub use adxl345::Adxl345;
pub use bank::Bank;
pub use i2c::{I2cDevice, I2cSlave, PointerSlave, Registers};
pub use ov5640::{Ov5640, pattern_line};
pub use readout::readout;
pub use spi::{SpiDevice, SpiSlave};
pub use ssd1306::Ssd1306;

/// A picture a device shows, tightly packed opaque RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// The pins a device drives into the PL, as seen from the PL's input pins of
/// the same Pmod port.
///
/// Open-drain and pulled-up lines (I2C) are "released" at level 1; a device
/// that drives a pin pulls it to 0 or (push-pull, SPI MISO) sets it to 1.
/// Pins no device drives read 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PinDrive {
    /// Bit *n* set: the device drives pin *n*.
    pub mask: u8,
    /// The level it drives (only bits in `mask` matter).
    pub levels: u8,
}

impl PinDrive {
    /// A device that drives nothing.
    pub const NONE: PinDrive = PinDrive { mask: 0, levels: 0 };
}

/// A physical quantity the host can set on a device (`temperature`, ...).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceValue {
    /// Name used in `[device_values]`, `pmod_set` and `--device-set`.
    pub name: &'static str,
    /// Current value.
    pub value: f64,
    /// Unit, for display (`degC`, `g`, `counts`).
    pub unit: &'static str,
    /// Smallest value the model represents; settings are clamped to it.
    pub min: f64,
    /// Largest value the model represents.
    pub max: f64,
}

/// A register (or other piece of internal state) of a device, for inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRegister {
    /// Register name.
    pub name: String,
    /// Address in the device's register map.
    pub addr: u32,
    /// Current content.
    pub value: u32,
}

/// A device on a pin group.
pub trait BoardDevice: Send {
    /// Short kind name (`"oled"`).
    fn kind(&self) -> &'static str;

    /// The pin levels the PL drives changed to `levels` at `t_ns` (PL time).
    fn pins(&mut self, t_ns: u64, levels: u8);

    /// The pins the device drives now. Read right after [`BoardDevice::pins`];
    /// the result reaches the PL in the same simulated instant.
    fn drive(&self) -> PinDrive {
        PinDrive::NONE
    }

    /// A device that drives the PL's video input (a camera) is asked for line
    /// `y` of frame `frame` (`w` pixels of a `h`-line picture) and answers
    /// with `3 * w` bytes of RGB, or `None` to let the host's video source
    /// through.
    fn video_line(&mut self, _frame: u32, _y: u32, _w: u32, _h: u32) -> Option<Vec<u8>> {
        None
    }

    /// What the device shows now, if it has a display.
    fn image(&self) -> Option<DeviceImage> {
        None
    }

    /// One line of state for logs and panels.
    fn describe(&self) -> String;

    /// The physical quantities the host may set, with their current values.
    fn values(&self) -> Vec<DeviceValue> {
        Vec::new()
    }

    /// Sets a physical quantity by name (ASCII case-insensitive). Values
    /// outside the model's range are clamped. Errors name what is accepted.
    fn set_value(&mut self, name: &str, _value: f64) -> Result<(), String> {
        Err(format!(
            "`{}` has no settable quantity `{name}`",
            self.kind()
        ))
    }

    /// The register file or other internal state, for inspection.
    fn registers(&self) -> Vec<DeviceRegister> {
        Vec::new()
    }
}

/// Device kinds `[devices]` accepts.
pub const KINDS: &[&str] = &["oled", "tmp2", "als", "acl", "pcam"];

/// Creates a device by kind.
pub fn create(kind: &str) -> Option<Box<dyn BoardDevice>> {
    match kind {
        "oled" => Some(Box::new(Ssd1306::new())),
        "tmp2" => Some(Box::new(Adt7420::new())),
        "als" => Some(Box::new(Adc081::new())),
        "acl" => Some(Box::new(Adxl345::new())),
        "pcam" => Some(Box::new(Ov5640::new())),
        _ => None,
    }
}

/// The camera connector's pin group (`cam` in `[devices]`); the Pmod ports
/// are groups 0..=4.
pub const CAMERA_PORT: u8 = 5;

/// True when a device of `kind` can plug into `port`: the Pcam into the
/// camera connector, every other module into a Pmod port JA..JE (0..=4).
pub fn fits_port(kind: &str, port: u8) -> bool {
    if kind == "pcam" {
        port == CAMERA_PORT
    } else {
        port < CAMERA_PORT
    }
}

/// The settable quantities of a device kind (empty for kinds without any, and
/// for unknown kinds).
pub fn value_names(kind: &str) -> Vec<&'static str> {
    create(kind)
        .map(|d| d.values().iter().map(|v| v.name).collect())
        .unwrap_or_default()
}

/// Sets `name` on `dev` after applying the shared lookup rules: the name is
/// matched case-insensitively against [`BoardDevice::values`], the value must
/// be finite and is clamped to the quantity's range. Returns the value stored.
pub fn set_checked(dev: &mut dyn BoardDevice, name: &str, value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err(format!("`{name}`: {value} is not a finite number"));
    }
    let values = dev.values();
    let v = values
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            format!(
                "`{}` has no quantity `{name}`; available: {}",
                dev.kind(),
                if values.is_empty() {
                    "(none)".to_string()
                } else {
                    values.iter().map(|v| v.name).collect::<Vec<_>>().join(", ")
                }
            )
        })?;
    let clamped = value.clamp(v.min, v.max);
    dev.set_value(v.name, clamped)?;
    Ok(clamped)
}
