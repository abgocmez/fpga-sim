use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The built-in Zybo Z7-20 description (`boards/zybo-z7-20.toml`).
pub const ZYBO_Z7_20_TOML: &str = include_str!("../../../boards/zybo-z7-20.toml");

/// Errors loading or validating a board description.
#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    /// The TOML did not parse or did not match the schema.
    #[error("board description is not valid: {0}")]
    Parse(String),
    /// The description parsed but is inconsistent.
    #[error("board description has {} problem(s):\n  - {}", .0.len(), .0.join("\n  - "))]
    Invalid(Vec<String>),
}

/// A drawing-space rectangle for the board.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Drawing width in layout units.
    pub width: f32,
    /// Drawing height in layout units.
    pub height: f32,
}

/// A slide switch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    /// Unique id, for example `sw0`.
    pub id: String,
    /// FPGA package pin (unverified, see the board file header).
    #[serde(default)]
    pub pin: Option<String>,
    /// Keyboard shortcut shown in the GUI.
    #[serde(default)]
    pub key: Option<String>,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
}

/// A push button.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Button {
    /// Unique id, for example `btn0`.
    pub id: String,
    /// FPGA package pin (unverified).
    #[serde(default)]
    pub pin: Option<String>,
    /// Keyboard shortcut shown in the GUI.
    #[serde(default)]
    pub key: Option<String>,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
}

/// User LED colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedColor {
    /// Green.
    #[default]
    Green,
    /// Red.
    Red,
    /// Yellow.
    Yellow,
    /// Blue.
    Blue,
}

/// A single-colour LED.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Led {
    /// Unique id, for example `led0`.
    pub id: String,
    /// FPGA package pin (unverified).
    #[serde(default)]
    pub pin: Option<String>,
    /// LED colour.
    #[serde(default)]
    pub color: LedColor,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
}

/// A PWM-driven RGB LED.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbLed {
    /// Unique id, for example `led5`.
    pub id: String,
    /// Red channel pin (unverified).
    #[serde(default)]
    pub pin_r: Option<String>,
    /// Green channel pin (unverified).
    #[serde(default)]
    pub pin_g: Option<String>,
    /// Blue channel pin (unverified).
    #[serde(default)]
    pub pin_b: Option<String>,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
}

/// Whether a Pmod is wired to the PL or to PS MIO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PmodDomain {
    /// Connected to PL pins.
    Pl,
    /// Connected to PS MIO pins.
    Ps,
}

/// A 12-pin Pmod port exposing eight signal pins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pmod {
    /// Unique id, for example `JA`.
    pub id: String,
    /// PL or PS.
    pub domain: PmodDomain,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
}

impl Pmod {
    /// Names of the eight signal pins: `<id>1`..`<id>4` and `<id>7`..`<id>10`.
    pub fn pin_names(&self) -> [String; 8] {
        let n = [1, 2, 3, 4, 7, 8, 9, 10];
        std::array::from_fn(|i| format!("{}{}", self.id, n[i]))
    }
}

/// A connector or major component drawn on the board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    /// Unique id, for example `hdmi_tx`.
    pub id: String,
    /// Kind of part (`ethernet`, `hdmi_rx`, `soc`, ...).
    pub kind: String,
    /// Label to display.
    pub label: String,
    /// Centre x in layout units.
    pub x: f32,
    /// Centre y in layout units.
    pub y: f32,
    /// Width in layout units.
    pub w: f32,
    /// Height in layout units.
    pub h: f32,
}

/// A complete board description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardDef {
    /// Schema version; currently 1.
    pub schema_version: u32,
    /// Machine name, for example `zybo-z7-20`.
    pub name: String,
    /// Human-readable name.
    pub display_name: String,
    /// FPGA part number.
    pub part: String,
    /// DDR size in MiB.
    pub ddr_mb: u32,
    /// PL oscillator frequency.
    pub pl_clock_hz: u64,
    /// PS clock frequency.
    pub ps_clock_hz: u64,
    /// `verified` once every pin has been checked against the schematic.
    #[serde(default = "default_pin_status")]
    pub pin_status: String,
    /// Drawing space.
    pub layout: Layout,
    /// Slide switches.
    #[serde(default)]
    pub switches: Vec<Switch>,
    /// Push buttons.
    #[serde(default)]
    pub buttons: Vec<Button>,
    /// User LEDs.
    #[serde(default)]
    pub leds: Vec<Led>,
    /// RGB LEDs.
    #[serde(default)]
    pub rgb_leds: Vec<RgbLed>,
    /// Pmod ports.
    #[serde(default)]
    pub pmods: Vec<Pmod>,
    /// Connectors and parts.
    #[serde(default)]
    pub connectors: Vec<Connector>,
}

fn default_pin_status() -> String {
    "unverified".to_string()
}

impl BoardDef {
    /// The built-in Zybo Z7-20.
    pub fn zybo_z7_20() -> BoardDef {
        BoardDef::from_toml(ZYBO_Z7_20_TOML)
            .unwrap_or_else(|e| unreachable!("built-in board file is covered by tests: {e}"))
    }

    /// Looks up a built-in board by machine name.
    pub fn builtin(name: &str) -> Option<BoardDef> {
        match name {
            "zybo-z7-20" => Some(BoardDef::zybo_z7_20()),
            _ => None,
        }
    }

    /// Parses and validates a board description.
    pub fn from_toml(text: &str) -> Result<BoardDef, BoardError> {
        let def: BoardDef = toml::from_str(text).map_err(|e| BoardError::Parse(e.to_string()))?;
        def.validate()?;
        Ok(def)
    }

    /// Checks internal consistency, reporting every problem found.
    pub fn validate(&self) -> Result<(), BoardError> {
        let mut problems = Vec::new();
        if self.schema_version != 1 {
            problems.push(format!(
                "schema_version is {}, only 1 is supported",
                self.schema_version
            ));
        }
        if self.layout.width <= 0.0 || self.layout.height <= 0.0 {
            problems.push("layout width and height must be positive".to_string());
        }
        if self.pl_clock_hz == 0 {
            problems.push("pl_clock_hz must be non-zero".to_string());
        }

        // (kind, id, x, y) for every placed item, in a stable order.
        let mut placed: Vec<(&str, &str, f32, f32)> = Vec::new();
        placed.extend(
            self.switches
                .iter()
                .map(|s| ("switch", s.id.as_str(), s.x, s.y)),
        );
        placed.extend(
            self.buttons
                .iter()
                .map(|b| ("button", b.id.as_str(), b.x, b.y)),
        );
        placed.extend(self.leds.iter().map(|l| ("led", l.id.as_str(), l.x, l.y)));
        placed.extend(
            self.rgb_leds
                .iter()
                .map(|l| ("rgb_led", l.id.as_str(), l.x, l.y)),
        );
        placed.extend(self.pmods.iter().map(|p| ("pmod", p.id.as_str(), p.x, p.y)));
        placed.extend(
            self.connectors
                .iter()
                .map(|c| ("connector", c.id.as_str(), c.x, c.y)),
        );

        let mut seen = HashSet::new();
        for (kind, id, x, y) in placed {
            if id.is_empty() {
                problems.push(format!("a {kind} has an empty id"));
            } else if !seen.insert(id) {
                problems.push(format!("duplicate id `{id}` ({kind})"));
            }
            if !(0.0..=self.layout.width).contains(&x) || !(0.0..=self.layout.height).contains(&y) {
                problems.push(format!(
                    "{kind} `{id}` at ({x}, {y}) is outside the {}x{} layout",
                    self.layout.width, self.layout.height
                ));
            }
        }
        for c in &self.connectors {
            if c.w <= 0.0 || c.h <= 0.0 {
                problems.push(format!("connector `{}` has a non-positive size", c.id));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(BoardError::Invalid(problems))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_zybo_parses_and_has_expected_parts() {
        let b = BoardDef::zybo_z7_20();
        assert_eq!(b.part, "xc7z020clg400-1");
        assert_eq!(b.switches.len(), 4);
        assert_eq!(b.buttons.len(), 4);
        assert_eq!(b.leds.len(), 4);
        assert_eq!(b.rgb_leds.len(), 2);
        assert_eq!(b.pmods.len(), 6);
        assert_eq!(b.pin_status, "unverified");
        // The Zybo Z7 has no VGA connector (prompt appendix §19).
        assert!(b.connectors.iter().all(|c| c.kind != "vga"));
        assert!(b.connectors.iter().any(|c| c.kind == "hdmi_rx"));
        assert!(b.connectors.iter().any(|c| c.kind == "hdmi_tx"));
    }

    #[test]
    fn pmod_pin_names_skip_power_and_ground() {
        let p = BoardDef::zybo_z7_20().pmods[0].clone();
        assert_eq!(
            p.pin_names().to_vec(),
            ["JA1", "JA2", "JA3", "JA4", "JA7", "JA8", "JA9", "JA10"]
        );
    }

    #[test]
    fn validation_reports_every_problem() {
        // A Windows checkout may have CRLF line ends.
        let text = ZYBO_Z7_20_TOML.replace("\r\n", "\n");
        let text = text.replace("id = \"sw1\"", "id = \"sw0\"");
        let text = text.replace("x = 160\ny = 515", "x = 5000\ny = 515");
        let err = BoardDef::from_toml(&text).unwrap_err().to_string();
        assert!(err.contains("duplicate id `sw0`"), "{err}");
        assert!(err.contains("outside the 1000x640 layout"), "{err}");
        assert!(err.contains("2 problem(s)"), "{err}");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = format!("{ZYBO_Z7_20_TOML}\nbogus_field = 1\n");
        // Appending at the end places it inside the last [[connectors]] table.
        assert!(matches!(
            BoardDef::from_toml(&text),
            Err(BoardError::Parse(_))
        ));
    }
}
