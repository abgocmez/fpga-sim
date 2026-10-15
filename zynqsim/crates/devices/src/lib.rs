//! Models of hardware that plugs into the board's pins.
//!
//! A device sees the levels of a Pmod port (eight signal pins, bit *n* = pin
//! *n*, in Pmod order 1, 2, 3, 4, 7, 8, 9, 10) with the simulated time of every
//! change, and may show a picture. Devices are pure Rust and independent of
//! the PS and PL backends: they only consume pin events.
//!
//! The models follow the datasheets as remembered by their author and have
//! **not** been compared with the hardware (`VERIFY:` in `STATUS.md`).

mod ssd1306;

pub mod font5x7 {
    //! The 5x7 font shared with the firmware examples.
    include!("font5x7.rs");
}

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

/// A device on a pin group.
pub trait BoardDevice: Send {
    /// Short kind name (`"oled"`).
    fn kind(&self) -> &'static str;

    /// The pin levels changed to `levels` at `t_ns` (PL time).
    fn pins(&mut self, t_ns: u64, levels: u8);

    /// What the device shows now, if it has a display.
    fn image(&self) -> Option<DeviceImage> {
        None
    }

    /// One line of state for logs and panels.
    fn describe(&self) -> String;
}

/// Device kinds `[devices]` accepts.
pub const KINDS: &[&str] = &["oled"];

/// Creates a device by kind.
pub fn create(kind: &str) -> Option<Box<dyn BoardDevice>> {
    match kind {
        "oled" => Some(Box::new(Ssd1306::new())),
        _ => None,
    }
}
