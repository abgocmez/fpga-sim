use crate::geom;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The built-in Zybo Z7-20 description (`boards/zybo-z7-20.toml`).
pub const ZYBO_Z7_20_TOML: &str = include_str!("../../../boards/zybo-z7-20.toml");
/// The built-in Zybo Z7-10 description (`boards/zybo-z7-10.toml`).
pub const ZYBO_Z7_10_TOML: &str = include_str!("../../../boards/zybo-z7-10.toml");
/// The built-in ZedBoard description (`boards/zedboard.toml`).
pub const ZEDBOARD_TOML: &str = include_str!("../../../boards/zedboard.toml");
/// The built-in PYNQ-Z2 description (`boards/pynq-z2.toml`).
pub const PYNQ_Z2_TOML: &str = include_str!("../../../boards/pynq-z2.toml");

/// Machine names and sources of the built-in boards, in listing order.
pub const BUILTIN_BOARDS: &[(&str, &str)] = &[
    ("zybo-z7-20", ZYBO_Z7_20_TOML),
    ("zybo-z7-10", ZYBO_Z7_10_TOML),
    ("zedboard", ZEDBOARD_TOML),
    ("pynq-z2", PYNQ_Z2_TOML),
];

/// Most slide switches, push buttons or user LEDs a board may have: the testbench
/// interface (`tel_ctrl` switches/buttons in, `tel_event` LED mask out) carries one byte
/// each, bit *n* for component *n* of the board file (ADR-022).
pub const MAX_BITS: usize = 8;
/// Most RGB LEDs (three PWM pins each in a six-bit pin group).
pub const MAX_RGB_LEDS: usize = 2;
/// Most Pmod ports (pin groups 0..=5; group 6 is the RGB LEDs).
pub const MAX_PMODS: usize = 6;

/// The `kind` values a connector may have. The renderer colours some of them; the others
/// are drawn as a generic grey part.
pub const CONNECTOR_KINDS: &[&str] = &[
    "soc", "ddr", "qspi", "ethernet", "usb_otg", "usb_host", "usb_uart", "hdmi_rx", "hdmi_tx",
    "vga", "microsd", "sd", "audio", "power", "pcam", "oled", "header", "fmc", "xadc", "jtag",
    "clock",
];

/// Errors loading or validating a board description.
#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    /// The TOML did not parse or did not match the schema.
    #[error("board description is not valid: {0}")]
    Parse(String),
    /// The description parsed but is inconsistent.
    #[error("board description has {} problem(s):\n  - {}", .0.len(), .0.join("\n  - "))]
    Invalid(Vec<String>),
    /// A board file or its artwork could not be read.
    #[error("{0}")]
    Io(String),
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

/// Informational resources of the board's FPGA (what the device offers, not what a
/// design uses). Surfaced by `zynqsim inspect`; nothing is checked against it yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    /// 6-input LUTs.
    #[serde(default)]
    pub luts: Option<u32>,
    /// Flip-flops.
    #[serde(default)]
    pub flip_flops: Option<u32>,
    /// 36 Kb block RAMs.
    #[serde(default)]
    pub bram_36k: Option<u32>,
    /// DSP48 slices.
    #[serde(default)]
    pub dsp_slices: Option<u32>,
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
    /// Optional artwork drawn behind the controls in the 2D view: the path of an SVG file,
    /// relative to the board file (see the `svg` module for the supported subset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
    /// The artwork's text, filled in when the board is loaded from a file.
    #[serde(skip)]
    pub svg_source: Option<String>,
    /// Informational FPGA resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
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

/// `[A-Za-z][A-Za-z0-9_]*`: ids double as pin names and `--at` targets.
fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic())
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

/// A placed item for the layout checks.
struct Placed<'a> {
    kind: &'static str,
    id: &'a str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl BoardDef {
    /// The built-in Zybo Z7-20.
    pub fn zybo_z7_20() -> BoardDef {
        BoardDef::builtin("zybo-z7-20")
            .unwrap_or_else(|| unreachable!("the Zybo Z7-20 is a built-in board"))
    }

    /// Looks up a built-in board by machine name.
    pub fn builtin(name: &str) -> Option<BoardDef> {
        BUILTIN_BOARDS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, text)| {
                BoardDef::from_toml(text).unwrap_or_else(|e| {
                    unreachable!("built-in board file is covered by tests: {e}")
                })
            })
    }

    /// The pin and control names this board answers to, as a short help line:
    /// `sw0..sw7, btn0..btn6, led0..led7, led4_r/g/b, led5_r/g/b, PMODA1..PMODB10`.
    pub fn pin_help(&self) -> String {
        fn range<'a>(ids: impl Iterator<Item = &'a str>) -> Option<String> {
            let v: Vec<&str> = ids.collect();
            match v.as_slice() {
                [] => None,
                [one] => Some((*one).to_string()),
                [first, .., last] => Some(format!("{first}..{last}")),
            }
        }
        let mut parts: Vec<String> = Vec::new();
        parts.extend(range(self.switches.iter().map(|s| s.id.as_str())));
        parts.extend(range(self.buttons.iter().map(|s| s.id.as_str())));
        parts.extend(range(self.leds.iter().map(|s| s.id.as_str())));
        parts.extend(self.rgb_leds.iter().map(|l| format!("{}_r/g/b", l.id)));
        if let (Some(first), Some(last)) = (self.pmods.first(), self.pmods.last()) {
            parts.push(format!("{}1..{}10", first.id, last.id));
        }
        parts.join(", ")
    }

    /// Machine names of the built-in boards.
    pub fn builtin_names() -> Vec<&'static str> {
        BUILTIN_BOARDS.iter().map(|(n, _)| *n).collect()
    }

    /// Parses and validates a board description. The artwork named by `svg`, if any, is
    /// not read (there is no directory to resolve it against); see
    /// [`BoardDef::from_toml_in`].
    pub fn from_toml(text: &str) -> Result<BoardDef, BoardError> {
        BoardDef::from_toml_in(text, None)
    }

    /// Like [`BoardDef::from_toml`]; with a directory, also reads the SVG artwork (relative
    /// to it) and checks it is in the supported subset.
    pub fn from_toml_in(text: &str, dir: Option<&Path>) -> Result<BoardDef, BoardError> {
        let mut def: BoardDef =
            toml::from_str(text).map_err(|e| BoardError::Parse(e.to_string()))?;
        def.validate()?;
        if let (Some(rel), Some(dir)) = (def.svg.as_deref(), dir) {
            let file = dir.join(rel);
            let svg = std::fs::read_to_string(&file).map_err(|e| {
                BoardError::Io(format!(
                    "cannot read the board artwork {}: {e}",
                    file.display()
                ))
            })?;
            crate::svg::parse(&svg).map_err(|e| {
                BoardError::Invalid(vec![format!("svg `{rel}` is not usable: {e}")])
            })?;
            def.svg_source = Some(svg);
        }
        Ok(def)
    }

    /// Reads, parses and validates a board file; its artwork is resolved next to it.
    pub fn load(path: &Path) -> Result<BoardDef, BoardError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            BoardError::Io(format!(
                "cannot read the board file {}: {e}",
                path.display()
            ))
        })?;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        BoardDef::from_toml_in(&text, Some(dir.unwrap_or_else(|| Path::new("."))))
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
        let name_ok = !self.name.is_empty()
            && self
                .name
                .starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !name_ok {
            problems.push(format!(
                "name `{}` must be lower-case letters, digits, `-` or `_`",
                self.name
            ));
        }
        if self.layout.width <= 0.0 || self.layout.height <= 0.0 {
            problems.push("layout width and height must be positive".to_string());
        }
        if self.pl_clock_hz == 0 {
            problems.push("pl_clock_hz must be non-zero".to_string());
        }
        if self.ddr_mb == 0 {
            problems.push("ddr_mb must be non-zero".to_string());
        }
        if self.svg.as_deref().is_some_and(|s| s.trim().is_empty()) {
            problems.push("svg is an empty path".to_string());
        }
        for (what, n, max) in [
            ("switches", self.switches.len(), MAX_BITS),
            ("buttons", self.buttons.len(), MAX_BITS),
            ("leds", self.leds.len(), MAX_BITS),
            ("rgb_leds", self.rgb_leds.len(), MAX_RGB_LEDS),
            ("pmods", self.pmods.len(), MAX_PMODS),
        ] {
            if n > max {
                problems.push(format!(
                    "{n} {what} but at most {max} are supported{}",
                    if max == MAX_BITS {
                        " (the testbench interface carries one byte per group)"
                    } else {
                        ""
                    }
                ));
            }
        }

        // Every placed item, in a stable order.
        let mut placed: Vec<Placed> = Vec::new();
        let mut add = |kind, id, x, y, w, h| {
            placed.push(Placed {
                kind,
                id,
                x,
                y,
                w,
                h,
            })
        };
        for s in &self.switches {
            add(
                "switch",
                s.id.as_str(),
                s.x,
                s.y,
                geom::SWITCH_W,
                geom::SWITCH_H,
            );
        }
        for b in &self.buttons {
            add(
                "button",
                b.id.as_str(),
                b.x,
                b.y,
                geom::BUTTON,
                geom::BUTTON,
            );
        }
        for l in &self.leds {
            let d = 2.0 * (geom::LED_R + 2.0);
            add("led", l.id.as_str(), l.x, l.y, d, d);
        }
        for l in &self.rgb_leds {
            let d = 2.0 * (geom::RGB_R + 2.0);
            add("rgb_led", l.id.as_str(), l.x, l.y, d, d);
        }
        for p in &self.pmods {
            add("pmod", p.id.as_str(), p.x, p.y, geom::PMOD_W, geom::PMOD_H);
        }
        for c in &self.connectors {
            add("connector", c.id.as_str(), c.x, c.y, c.w, c.h);
        }

        // Ids share one namespace with the names derived from them (RGB channels, Pmod
        // pins): all of them are accepted by `pin_get` and `--at`.
        let mut names: HashMap<String, String> = HashMap::new();
        let (lw, lh) = (self.layout.width, self.layout.height);
        for p in &placed {
            if p.id.is_empty() {
                problems.push(format!("a {} has an empty id", p.kind));
            } else if !is_ident(p.id) {
                problems.push(format!(
                    "{} id `{}` must be a letter followed by letters, digits or `_`",
                    p.kind, p.id
                ));
            } else if names
                .insert(p.id.to_string(), format!("{} `{}`", p.kind, p.id))
                .is_some()
            {
                problems.push(format!("duplicate id `{}` ({})", p.id, p.kind));
            }
            if !(0.0..=lw).contains(&p.x) || !(0.0..=lh).contains(&p.y) {
                problems.push(format!(
                    "{} `{}` at ({}, {}) is outside the {lw}x{lh} layout",
                    p.kind, p.id, p.x, p.y
                ));
            } else if p.x - p.w / 2.0 < 0.0
                || p.y - p.h / 2.0 < 0.0
                || p.x + p.w / 2.0 > lw
                || p.y + p.h / 2.0 > lh
            {
                problems.push(format!(
                    "{} `{}` ({}x{} at ({}, {})) extends beyond the {lw}x{lh} layout",
                    p.kind, p.id, p.w, p.h, p.x, p.y
                ));
            }
        }
        let mut derived = |name: String, owner: String| {
            if let Some(prev) = names.insert(name.clone(), owner.clone()) {
                problems.push(format!(
                    "name `{name}` of {owner} is already used by {prev}"
                ));
            }
        };
        for l in &self.rgb_leds {
            for s in ["_r", "_g", "_b"] {
                derived(format!("{}{s}", l.id), format!("rgb_led `{}`", l.id));
            }
        }
        for p in &self.pmods {
            for n in p.pin_names() {
                derived(n, format!("pmod `{}`", p.id));
            }
        }

        for c in &self.connectors {
            if c.w <= 0.0 || c.h <= 0.0 {
                problems.push(format!("connector `{}` has a non-positive size", c.id));
            }
            if !CONNECTOR_KINDS.contains(&c.kind.as_str()) {
                problems.push(format!(
                    "connector `{}` has unknown kind `{}`; known kinds: {}",
                    c.id,
                    c.kind,
                    CONNECTOR_KINDS.join(", ")
                ));
            }
        }

        // FPGA package pins: syntax, and no pin on two parts.
        let mut pins: HashMap<String, String> = HashMap::new();
        let mut check_pin = |pin: &Option<String>, owner: String| {
            let Some(p) = pin else { return };
            if !is_ident(p) {
                problems.push(format!(
                    "pin `{p}` of {owner} must be a letter followed by letters or digits"
                ));
            } else if let Some(prev) = pins.insert(p.to_ascii_uppercase(), owner.clone()) {
                problems.push(format!("pin `{p}` is used by both {prev} and {owner}"));
            }
        };
        for s in &self.switches {
            check_pin(&s.pin, format!("switch `{}`", s.id));
        }
        for b in &self.buttons {
            check_pin(&b.pin, format!("button `{}`", b.id));
        }
        for l in &self.leds {
            check_pin(&l.pin, format!("led `{}`", l.id));
        }
        for l in &self.rgb_leds {
            check_pin(&l.pin_r, format!("rgb_led `{}` (r)", l.id));
            check_pin(&l.pin_g, format!("rgb_led `{}` (g)", l.id));
            check_pin(&l.pin_b, format!("rgb_led `{}` (b)", l.id));
        }

        // Keyboard shortcuts: one letter or digit, unique.
        let mut keys: HashMap<String, String> = HashMap::new();
        let keyed = self
            .switches
            .iter()
            .map(|s| ("switch", s.id.as_str(), s.key.as_deref()))
            .chain(
                self.buttons
                    .iter()
                    .map(|b| ("button", b.id.as_str(), b.key.as_deref())),
            );
        for (kind, id, key) in keyed {
            let Some(k) = key else { continue };
            if k.chars().count() != 1 || !k.chars().all(|c| c.is_ascii_alphanumeric()) {
                problems.push(format!(
                    "{kind} `{id}` key `{k}` must be a single letter or digit"
                ));
            } else if let Some(prev) = keys.insert(k.to_ascii_uppercase(), format!("{kind} `{id}`"))
            {
                problems.push(format!(
                    "key `{k}` is bound to both {prev} and {kind} `{id}`"
                ));
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

    fn counts(b: &BoardDef) -> [usize; 5] {
        [
            b.switches.len(),
            b.buttons.len(),
            b.leds.len(),
            b.rgb_leds.len(),
            b.pmods.len(),
        ]
    }

    #[test]
    fn every_builtin_board_loads_with_its_component_counts() {
        // The counts are from memory of the vendors' documents (VERIFY in the files); what
        // the test proves is that each file loads and sizes its vectors from itself.
        let want = [
            ("zybo-z7-20", [4, 4, 4, 2, 6]),
            ("zybo-z7-10", [4, 4, 4, 2, 6]),
            ("zedboard", [8, 7, 8, 0, 4]),
            ("pynq-z2", [2, 4, 4, 2, 2]),
        ];
        assert_eq!(
            BoardDef::builtin_names(),
            want.iter().map(|w| w.0).collect::<Vec<_>>()
        );
        for (name, c) in want {
            let b = BoardDef::builtin(name).unwrap_or_else(|| panic!("{name} is built in"));
            assert_eq!(b.name, name);
            assert_eq!(counts(&b), c, "{name}");
            assert_eq!(b.pin_status, "unverified", "{name}: nothing is checked");
            let s = crate::BoardState::new(&b);
            assert_eq!(
                (s.switches.len(), s.buttons.len(), s.leds.len(), s.rgb.len()),
                (c[0], c[1], c[2], c[3]),
                "{name}: state is sized by the definition"
            );
        }
        assert!(BoardDef::builtin("nope").is_none());
    }

    #[test]
    fn the_z7_10_is_the_z7_20_with_a_smaller_device() {
        let (a, b) = (
            BoardDef::builtin("zybo-z7-20").unwrap(),
            BoardDef::builtin("zybo-z7-10").unwrap(),
        );
        assert_eq!(
            (a.part.as_str(), b.part.as_str()),
            ("xc7z020clg400-1", "xc7z010clg400-1")
        );
        assert_eq!(
            (a.switches.clone(), a.leds.clone()),
            (b.switches.clone(), b.leds.clone())
        );
        assert_eq!(a.connectors.len(), b.connectors.len());
        let (ra, rb) = (a.resources.unwrap(), b.resources.unwrap());
        assert!(rb.luts < ra.luts && rb.bram_36k < ra.bram_36k && rb.dsp_slices < ra.dsp_slices);
    }

    #[test]
    fn the_new_boards_keep_their_controls_apart() {
        // No two controls (switches, buttons, LEDs, RGB LEDs, Pmods) overlap, so a click is
        // never ambiguous; connectors are background parts and may be anywhere.
        for name in BoardDef::builtin_names() {
            let b = BoardDef::builtin(name).unwrap();
            let mut boxes: Vec<(String, f32, f32, f32, f32)> = Vec::new();
            for s in &b.switches {
                boxes.push((s.id.clone(), s.x, s.y, geom::SWITCH_W, geom::SWITCH_H));
            }
            for s in &b.buttons {
                boxes.push((s.id.clone(), s.x, s.y, geom::BUTTON, geom::BUTTON));
            }
            for s in &b.leds {
                boxes.push((s.id.clone(), s.x, s.y, 2.0 * geom::LED_R, 2.0 * geom::LED_R));
            }
            for s in &b.rgb_leds {
                boxes.push((s.id.clone(), s.x, s.y, 2.0 * geom::RGB_R, 2.0 * geom::RGB_R));
            }
            for s in &b.pmods {
                boxes.push((s.id.clone(), s.x, s.y, geom::PMOD_W, geom::PMOD_H));
            }
            for (i, a) in boxes.iter().enumerate() {
                for c in &boxes[i + 1..] {
                    let apart = (a.1 - c.1).abs() >= (a.3 + c.3) / 2.0
                        || (a.2 - c.2).abs() >= (a.4 + c.4) / 2.0;
                    assert!(apart, "{name}: {} overlaps {}", a.0, c.0);
                }
            }
        }
    }

    #[test]
    fn pin_help_lists_the_boards_names() {
        assert_eq!(
            BoardDef::zybo_z7_20().pin_help(),
            "sw0..sw3, btn0..btn3, led0..led3, led5_r/g/b, led6_r/g/b, JA1..JF10"
        );
        assert_eq!(
            BoardDef::builtin("zedboard").unwrap().pin_help(),
            "sw0..sw7, btn0..btn6, led0..led7, JA1..JD10"
        );
    }

    /// The Zybo file with `from` replaced by `to`, as validation sees it.
    fn problems_with(from: &str, to: &str) -> String {
        let text = ZYBO_Z7_20_TOML.replace("\r\n", "\n");
        assert!(text.contains(from), "the test edit `{from}` applies");
        match BoardDef::from_toml(&text.replacen(from, to, 1)) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn each_class_of_mistake_has_its_own_precise_message() {
        // unknown component kind
        let e = problems_with("kind = \"hdmi_rx\"", "kind = \"hdmi_rxx\"");
        assert!(
            e.contains("connector `hdmi_rx` has unknown kind `hdmi_rxx`; known kinds: soc, ddr"),
            "{e}"
        );
        // unknown table
        let e = problems_with("[[switches]]", "[[sliders]]\nid = \"s\"\n\n[[switches]]");
        assert!(e.contains("unknown field `sliders`"), "{e}");
        // unknown LED colour
        let e = problems_with("color = \"green\"", "color = \"mauve\"");
        assert!(e.contains("unknown variant `mauve`"), "{e}");
        // a control outside the layout
        let e = problems_with("x = 160\ny = 585", "x = 1160\ny = 585");
        assert!(
            e.contains("switch `sw0` at (1160, 585) is outside the 1000x640 layout"),
            "{e}"
        );
        // a control hanging over the edge
        let e = problems_with("x = 160\ny = 585", "x = 10\ny = 585");
        assert!(
            e.contains("switch `sw0` (36x64 at (10, 585)) extends beyond the 1000x640 layout"),
            "{e}"
        );
        // duplicate id, across kinds
        let e = problems_with("id = \"btn0\"", "id = \"sw0\"");
        assert!(e.contains("duplicate id `sw0` (button)"), "{e}");
        // an id that is not a valid name
        let e = problems_with("id = \"btn0\"", "id = \"btn 0\"");
        assert!(e.contains("button id `btn 0` must be a letter"), "{e}");
        // a derived name colliding with an id
        let e = problems_with("id = \"btn0\"", "id = \"JA1\"");
        assert!(
            e.contains("name `JA1` of pmod `JA` is already used by button `JA1`"),
            "{e}"
        );
        // one package pin on two parts
        let e = problems_with("pin = \"P15\"", "pin = \"G15\"");
        assert!(
            e.contains("pin `G15` is used by both switch `sw0` and switch `sw1`"),
            "{e}"
        );
        // keys
        let e = problems_with("key = \"2\"", "key = \"1\"");
        assert!(
            e.contains("key `1` is bound to both switch `sw0` and switch `sw1`"),
            "{e}"
        );
        let e = problems_with("key = \"2\"", "key = \"F2\"");
        assert!(
            e.contains("switch `sw1` key `F2` must be a single letter or digit"),
            "{e}"
        );
        // name syntax, schema version
        let e = problems_with("name = \"zybo-z7-20\"", "name = \"Zybo 20\"");
        assert!(e.contains("name `Zybo 20` must be lower-case"), "{e}");
        let e = problems_with("schema_version = 1", "schema_version = 2");
        assert!(
            e.contains("schema_version is 2, only 1 is supported"),
            "{e}"
        );
        // not TOML at all
        assert!(matches!(
            BoardDef::from_toml("this is = = not toml"),
            Err(BoardError::Parse(_))
        ));
        // clean file: the helper reports nothing
        assert_eq!(problems_with("id = \"sw0\"", "id = \"sw0\""), "");
    }

    #[test]
    fn more_than_eight_bits_do_not_fit_the_testbench_interface() {
        let mut b = BoardDef::builtin("zedboard").unwrap();
        let extra = Switch {
            id: "sw8".into(),
            pin: None,
            key: None,
            x: 556.0,
            y: 585.0,
        };
        b.switches.push(extra);
        let e = b.validate().unwrap_err().to_string();
        assert!(
            e.contains("9 switches but at most 8 are supported (the testbench interface"),
            "{e}"
        );
        let mut b = BoardDef::zybo_z7_20();
        b.rgb_leds.push(b.rgb_leds[0].clone());
        b.rgb_leds.last_mut().unwrap().id = "led7".into();
        let e = b.validate().unwrap_err().to_string();
        assert!(e.contains("3 rgb_leds but at most 2"), "{e}");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("zynqsim-board-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lab_board_toml(svg_line: &str) -> String {
        format!(
            "schema_version = 1\nname = \"lab\"\ndisplay_name = \"Lab\"\npart = \"xc7z020clg400-1\"\n\
             ddr_mb = 256\npl_clock_hz = 100000000\nps_clock_hz = 33333333\n{svg_line}\n\
             [layout]\nwidth = 200\nheight = 100\n\
             [[leds]]\nid = \"l0\"\nx = 50\ny = 50\n"
        )
    }

    #[test]
    fn a_board_file_loads_with_its_svg_and_reports_a_bad_one() {
        let d = scratch("load");
        let board = d.join("lab.toml");
        std::fs::write(&board, lab_board_toml("svg = \"lab.svg\"")).unwrap();

        // The artwork is missing.
        let e = BoardDef::load(&board).unwrap_err();
        assert!(matches!(e, BoardError::Io(_)), "{e}");
        assert!(
            e.to_string().contains("cannot read the board artwork"),
            "{e}"
        );

        // The artwork uses something outside the subset.
        std::fs::write(
            d.join("lab.svg"),
            r#"<svg viewBox="0 0 10 10"><g transform="scale(2)"/></svg>"#,
        )
        .unwrap();
        let e = BoardDef::load(&board).unwrap_err().to_string();
        assert!(
            e.contains("svg `lab.svg` is not usable") && e.contains("transform"),
            "{e}"
        );

        // A good one is read and kept.
        std::fs::write(
            d.join("lab.svg"),
            r##"<svg viewBox="0 0 10 10"><rect width="10" height="10" fill="#123456"/></svg>"##,
        )
        .unwrap();
        let b = BoardDef::load(&board).unwrap();
        assert_eq!(b.name, "lab");
        assert!(
            b.svg_source
                .as_deref()
                .is_some_and(|s| s.contains("#123456"))
        );
        assert_eq!(b.leds.len(), 1);

        // A missing board file is an I/O error naming the path.
        let e = BoardDef::load(&d.join("nope.toml"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("cannot read the board file") && e.contains("nope.toml"),
            "{e}"
        );

        // Without a directory the artwork is not read, and the board still validates.
        let b = BoardDef::from_toml(&lab_board_toml("svg = \"lab.svg\"")).unwrap();
        assert_eq!(
            (b.svg.as_deref(), b.svg_source.as_deref()),
            (Some("lab.svg"), None)
        );
        let _ = std::fs::remove_dir_all(&d);
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
