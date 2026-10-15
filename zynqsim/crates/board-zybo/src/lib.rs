//! Board definition, live board state, software renderer and hit-testing for
//! the Digilent Zybo Z7-20 (and any board described by the same TOML schema).
//!
//! The renderer is a plain software rasteriser so that the GUI view, the MCP
//! `board_screenshot` tool and the golden-image tests all show the same pixels.

mod def;
mod render;
mod scene3d;
mod state;

pub use def::{
    BoardDef, BoardError, Button, Connector, Layout, Led, LedColor, Pmod, PmodDomain, RgbLed,
    Switch, ZYBO_Z7_20_TOML,
};
pub use render::{BoardImage, Hit, decode_png_rgb, encode_rgba_png, hit_test, render, render_png};
pub use scene3d::{Camera, Scene, render_3d};
pub use state::{BoardState, PinLevel};

/// Geometry constants shared by the renderer and hit-testing, in layout units.
pub mod geom {
    /// Slide switch body width.
    pub const SWITCH_W: f32 = 36.0;
    /// Slide switch body height.
    pub const SWITCH_H: f32 = 64.0;
    /// Push button side length.
    pub const BUTTON: f32 = 48.0;
    /// User LED radius.
    pub const LED_R: f32 = 13.0;
    /// RGB LED radius.
    pub const RGB_R: f32 = 16.0;
    /// Pmod connector width.
    pub const PMOD_W: f32 = 120.0;
    /// Pmod connector height.
    pub const PMOD_H: f32 = 44.0;
}
