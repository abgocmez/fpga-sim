//! A Digilent Pmod SSD: two seven-segment digits, multiplexed by the PL.
//!
//! Pmod pin order of the eight levels (`VERIFY:` against the Digilent reference
//! manual): bit0 AA (segment a), bit1 AB (b), bit2 AC (c), bit3 AD (d), bit4 AE
//! (e), bit5 AF (f), bit6 AG (g), bit7 C (digit select: 0 = left digit,
//! 1 = right digit). Segments are active high here (`VERIFY:` polarity too).
//!
//! The PL shows one digit at a time, switching the select line fast. The
//! model keeps, for each digit, the segment pattern that was on while that
//! digit was selected. The selected digit shows the live pattern; the other
//! digit keeps its last pattern for [`PERSIST_NS`] after it was last selected
//! (the eye's persistence), then goes dark. Time is the PL time of the last pin
//! change, so a static display that stops changing keeps what it showed.
//!
//! Not modelled: the digit's own timing and brightness, a decimal point (none
//! on the pin list), patterns that are not hexadecimal digits are kept and
//! drawn as segments but read as no digit.
//! Covered by unit tests only: no end-to-end RTL test exists for this module.

use crate::{BoardDevice, DeviceImage, DeviceRegister};

/// How long a digit that is not selected keeps its pattern on the picture.
pub const PERSIST_NS: u64 = 4_000_000;

const SEG_MASK: u8 = 0x7F;
const SELECT: u8 = 1 << 7;

const W: usize = 128;
const H: usize = 32;
/// Digit box: width, height, stroke, and the middle bar's row.
const DW: usize = 44;
const DH: usize = 28;
const T: usize = 4;
const MID: usize = DH / 2;
/// Left x of the two digit boxes.
const DIGIT_X: [usize; 2] = [8, 76];

/// Segment bits: a = 1, b = 2, c = 4, d = 8, e = 16, f = 32, g = 64.
const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 4;
const D: u8 = 8;
const E: u8 = 16;
const F: u8 = 32;
const G: u8 = 64;

/// The hexadecimal digits and the segment patterns that show them.
pub const GLYPHS: [(u8, char); 16] = [
    (A | B | C | D | E | F, '0'),
    (B | C, '1'),
    (A | B | D | E | G, '2'),
    (A | B | C | D | G, '3'),
    (B | C | F | G, '4'),
    (A | C | D | F | G, '5'),
    (A | C | D | E | F | G, '6'),
    (A | B | C, '7'),
    (A | B | C | D | E | F | G, '8'),
    (A | B | C | D | F | G, '9'),
    (A | B | C | E | F | G, 'A'),
    (C | D | E | F | G, 'B'),
    (A | D | E | F, 'C'),
    (B | C | D | E | G, 'D'),
    (A | D | E | F | G, 'E'),
    (A | E | F | G, 'F'),
];

/// The character a segment pattern shows, if it is a hexadecimal digit.
pub fn decode(segments: u8) -> Option<char> {
    if segments == 0 {
        return None;
    }
    GLYPHS.iter().find(|(s, _)| *s == segments).map(|(_, c)| *c)
}

/// The two-digit display of a Pmod SSD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ssd {
    /// The levels the PL drives now.
    levels: u8,
    /// PL time of the last pin change.
    now: u64,
    /// Last segment pattern seen while each digit was selected.
    last: [u8; 2],
    /// When each digit was last selected (`None`: never).
    seen: [Option<u64>; 2],
    changes: u32,
}

impl Default for Ssd {
    fn default() -> Ssd {
        Ssd::new()
    }
}

impl Ssd {
    /// A display with both digits dark.
    pub fn new() -> Ssd {
        Ssd {
            levels: 0,
            now: 0,
            last: [0; 2],
            seen: [None; 2],
            changes: 0,
        }
    }

    /// The digit currently selected (0 = left, 1 = right).
    pub fn selected(&self) -> usize {
        usize::from(self.levels & SELECT != 0)
    }

    /// The segment pattern digit `d` shows now (0 when dark).
    pub fn segments(&self, d: usize) -> u8 {
        if d == self.selected() {
            return self.levels & SEG_MASK;
        }
        match self.seen[d] {
            Some(t) if self.now.saturating_sub(t) <= PERSIST_NS => self.last[d],
            _ => 0,
        }
    }

    /// The characters on the two digits; `None` for a dark digit or a
    /// pattern that is not a hexadecimal digit.
    pub fn digits(&self) -> [Option<char>; 2] {
        [decode(self.segments(0)), decode(self.segments(1))]
    }
}

/// Draws one digit's seven segments into `rgba` at `x0`.
fn draw_digit(rgba: &mut [u8], x0: usize, segments: u8) {
    // (x, y, w, h) of each segment, relative to the digit box.
    let rects: [(usize, usize, usize, usize); 7] = [
        (T, 0, DW - 2 * T, T),           // a
        (DW - T, T, T, MID - T),         // b
        (DW - T, MID, T, MID - T),       // c
        (T, DH - T, DW - 2 * T, T),      // d
        (0, MID, T, MID - T),            // e
        (0, T, T, MID - T),              // f
        (T, MID - T / 2, DW - 2 * T, T), // g
    ];
    for (bit, &(rx, ry, rw, rh)) in rects.iter().enumerate() {
        let on = segments >> bit & 1 != 0;
        let px = if on {
            [255, 200, 60, 255]
        } else {
            [30, 32, 40, 255]
        };
        for y in ry..ry + rh {
            for x in rx..rx + rw {
                let i = ((y + 2) * W + x0 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&px);
            }
        }
    }
}

impl BoardDevice for Ssd {
    fn kind(&self) -> &'static str {
        "ssd"
    }

    fn pins(&mut self, t_ns: u64, levels: u8) {
        self.levels = levels;
        self.now = t_ns;
        let sel = self.selected();
        self.last[sel] = levels & SEG_MASK;
        self.seen[sel] = Some(t_ns);
        self.changes += 1;
    }

    fn image(&self) -> Option<DeviceImage> {
        let mut rgba = Vec::with_capacity(W * H * 4);
        for _ in 0..W * H {
            rgba.extend_from_slice(&[6, 8, 14, 255]);
        }
        for (d, x0) in DIGIT_X.iter().enumerate() {
            draw_digit(&mut rgba, *x0, self.segments(d));
        }
        Some(DeviceImage {
            width: W as u32,
            height: H as u32,
            rgba,
        })
    }

    fn describe(&self) -> String {
        let show = |c: Option<char>| c.map_or("-".to_string(), |c| c.to_string());
        let [l, r] = self.digits();
        format!(
            "Pmod SSD: digits {}{} (segments {:07b} / {:07b}), {} pin change(s)",
            show(l),
            show(r),
            self.segments(0),
            self.segments(1),
            self.changes
        )
    }

    fn registers(&self) -> Vec<DeviceRegister> {
        vec![
            DeviceRegister {
                name: "DIGIT0".to_string(),
                addr: 0,
                value: u32::from(self.segments(0)),
            },
            DeviceRegister {
                name: "DIGIT1".to_string(),
                addr: 1,
                value: u32::from(self.segments(1)),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin levels of a segment pattern on digit `d`.
    fn lv(d: usize, segments: u8) -> u8 {
        segments | if d == 1 { SELECT } else { 0 }
    }

    fn lit(img: &DeviceImage) -> usize {
        img.rgba.chunks(4).filter(|p| p[0] > 200).count()
    }

    #[test]
    fn every_hex_digit_decodes_from_its_pattern() {
        for (i, (seg, ch)) in GLYPHS.iter().enumerate() {
            let mut d = Ssd::new();
            d.pins(i as u64 * 10, lv(0, *seg));
            assert_eq!(d.digits()[0], Some(*ch), "pattern {seg:07b}");
            assert_eq!(d.digits()[1], None, "the right digit was never selected");
        }
        assert_eq!(decode(0), None, "dark");
        assert_eq!(decode(A | B), None, "not a hexadecimal digit");
        assert_eq!(GLYPHS.len(), 16);
    }

    #[test]
    fn multiplexed_digits_show_together() {
        let mut d = Ssd::new();
        d.pins(0, lv(0, 0x06)); // left: 1
        d.pins(100_000, lv(1, 0x4F)); // right: 3, 100 us later
        assert_eq!(d.digits(), [Some('1'), Some('3')]);
        // Switching back: the right digit keeps its pattern.
        d.pins(200_000, lv(0, 0x06));
        assert_eq!(d.digits(), [Some('1'), Some('3')]);
        assert_eq!(d.selected(), 0);
    }

    #[test]
    fn a_deselected_digit_persists_then_goes_dark() {
        let mut d = Ssd::new();
        d.pins(0, lv(0, 0x7F)); // left: 8
        d.pins(1_000_000, lv(1, 0)); // right selected, blank; left persists
        assert_eq!(d.digits()[0], Some('8'), "1 ms after: still on the picture");
        d.pins(PERSIST_NS, lv(1, 0));
        assert_eq!(d.digits()[0], Some('8'), "exactly at the limit: still on");
        d.pins(PERSIST_NS + 1_000_001, lv(1, 0));
        assert_eq!(d.digits()[0], None, "after the persistence time: dark");
    }

    #[test]
    fn the_selected_digit_is_live_and_a_static_display_keeps_its_picture() {
        let mut d = Ssd::new();
        d.pins(0, lv(0, 0x3F)); // left: 0, selected
        // No further pin change for a long time: the selected digit stays lit.
        assert_eq!(d.digits()[0], Some('0'));
        assert_eq!(d.segments(0), 0x3F);
    }

    #[test]
    fn the_picture_shows_both_digits_and_is_blank_when_dark() {
        let mut d = Ssd::new();
        let dark = d.image().unwrap();
        assert_eq!((dark.width, dark.height), (128, 32));
        assert_eq!(dark.rgba.len(), 128 * 32 * 4);
        assert_eq!(lit(&dark), 0);
        d.pins(0, lv(0, 0x7F));
        d.pins(10, lv(1, 0x7F));
        let eights = lit(&d.image().unwrap());
        let mut one = Ssd::new();
        one.pins(0, lv(0, 0x06));
        one.pins(10, lv(1, 0x06));
        let ones = lit(&one.image().unwrap());
        assert!(ones > 0, "digits are drawn");
        assert!(eights > ones, "an 8 has more lit segments than a 1");
        assert!(d.describe().contains("88"));
    }
}
