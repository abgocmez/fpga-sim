//! A 128x32 SSD1306-class OLED on a Digilent-style Pmod OLED (SPI).
//!
//! Pmod pin order of the eight levels: bit0 CS (active low), bit1 SDIN (MOSI),
//! bit2 unused, bit3 SCLK, bit4 D/C (low = command), bit5 RES (active low),
//! bit6 VBATC, bit7 VDDC. Bytes are shifted MSB first and sampled on the
//! rising edge of SCLK while CS is low (SPI mode 0).
//!
//! Modelled: command and data writes, the three addressing modes with column
//! and page windows, display on/off, entire-display-on, inverse, segment
//! remap, COM scan direction, contrast, reset. Not modelled: scrolling,
//! start line / display offset, the charge pump and panel power sequencing
//! (the supply pins are recorded, not interpreted), timing of the panel.

use crate::{BoardDevice, DeviceImage};

const COLS: usize = 128;
const PAGES: usize = 4;
const ROWS: usize = PAGES * 8;

/// Pin bit positions.
const CS: u8 = 1 << 0;
const SDIN: u8 = 1 << 1;
const SCLK: u8 = 1 << 3;
const DC: u8 = 1 << 4;
const RES: u8 = 1 << 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Addressing {
    Horizontal,
    Vertical,
    Page,
}

/// What the next command byte is an argument of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    Contrast,
    Mode,
    ColStart,
    ColEnd,
    PageStart,
    PageEnd,
    /// A one-byte argument that is read and ignored.
    Skip,
}

/// The display controller.
#[derive(Clone)]
pub struct Ssd1306 {
    ram: [[u8; COLS]; PAGES],
    pins: u8,
    shift: u8,
    bits: u8,
    mode: Addressing,
    col: usize,
    page: usize,
    col_lo: usize,
    col_hi: usize,
    page_lo: usize,
    page_hi: usize,
    pending: Pending,
    on: bool,
    all_on: bool,
    inverse: bool,
    seg_remap: bool,
    com_flip: bool,
    contrast: u8,
    commands: u32,
    data_bytes: u32,
    vbatc: bool,
    vddc: bool,
}

impl Default for Ssd1306 {
    fn default() -> Ssd1306 {
        Ssd1306::new()
    }
}

impl Ssd1306 {
    /// A controller in its power-on state (display off, RAM cleared).
    pub fn new() -> Ssd1306 {
        Ssd1306 {
            ram: [[0; COLS]; PAGES],
            pins: CS | RES, // deselected, not in reset
            shift: 0,
            bits: 0,
            mode: Addressing::Page,
            col: 0,
            page: 0,
            col_lo: 0,
            col_hi: COLS - 1,
            page_lo: 0,
            page_hi: PAGES - 1,
            pending: Pending::None,
            on: false,
            all_on: false,
            inverse: false,
            seg_remap: false,
            com_flip: false,
            contrast: 0x7F,
            commands: 0,
            data_bytes: 0,
            vbatc: false,
            vddc: false,
        }
    }

    /// Whether the panel is switched on.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Display RAM, `[page][column]`, bit 0 of a byte is the top pixel.
    pub fn ram(&self) -> &[[u8; COLS]; PAGES] {
        &self.ram
    }

    /// Number of command and data bytes received.
    pub fn counters(&self) -> (u32, u32) {
        (self.commands, self.data_bytes)
    }

    fn reset(&mut self) {
        let keep_ram = self.ram;
        *self = Ssd1306::new();
        // A reset pulse does not clear the display RAM on real parts.
        self.ram = keep_ram;
        self.pins &= !RES;
    }

    fn command(&mut self, b: u8) {
        self.commands += 1;
        match self.pending {
            Pending::Contrast => {
                self.contrast = b;
                self.pending = Pending::None;
                return;
            }
            Pending::Mode => {
                self.mode = match b & 3 {
                    0 => Addressing::Horizontal,
                    1 => Addressing::Vertical,
                    _ => Addressing::Page,
                };
                self.pending = Pending::None;
                return;
            }
            Pending::ColStart => {
                self.col_lo = usize::from(b & 0x7F);
                self.pending = Pending::ColEnd;
                return;
            }
            Pending::ColEnd => {
                self.col_hi = usize::from(b & 0x7F);
                self.col = self.col_lo;
                self.pending = Pending::None;
                return;
            }
            Pending::PageStart => {
                self.page_lo = usize::from(b & 7).min(PAGES - 1);
                self.pending = Pending::PageEnd;
                return;
            }
            Pending::PageEnd => {
                self.page_hi = usize::from(b & 7).min(PAGES - 1);
                self.page = self.page_lo;
                self.pending = Pending::None;
                return;
            }
            Pending::Skip => {
                self.pending = Pending::None;
                return;
            }
            Pending::None => {}
        }
        match b {
            0x00..=0x0F => self.col = (self.col & !0x0F) | usize::from(b & 0x0F),
            0x10..=0x1F => self.col = (self.col & 0x0F) | (usize::from(b & 0x0F) << 4),
            0x20 => self.pending = Pending::Mode,
            0x21 => self.pending = Pending::ColStart,
            0x22 => self.pending = Pending::PageStart,
            0x40..=0x7F => {} // display start line: not modelled
            0x81 => self.pending = Pending::Contrast,
            0x8D | 0xA8 | 0xD3 | 0xD5 | 0xD9 | 0xDA | 0xDB => self.pending = Pending::Skip,
            0xA0 => self.seg_remap = false,
            0xA1 => self.seg_remap = true,
            0xA4 => self.all_on = false,
            0xA5 => self.all_on = true,
            0xA6 => self.inverse = false,
            0xA7 => self.inverse = true,
            0xAE => self.on = false,
            0xAF => self.on = true,
            0xB0..=0xB7 => self.page = usize::from(b & 7).min(PAGES - 1),
            0xC0 => self.com_flip = false,
            0xC8 => self.com_flip = true,
            _ => {} // nop and the rest
        }
    }

    fn data(&mut self, b: u8) {
        self.data_bytes += 1;
        self.ram[self.page][self.col.min(COLS - 1)] = b;
        match self.mode {
            Addressing::Page => {
                self.col = (self.col + 1) % COLS;
            }
            Addressing::Horizontal => {
                if self.col >= self.col_hi {
                    self.col = self.col_lo;
                    self.page = if self.page >= self.page_hi {
                        self.page_lo
                    } else {
                        self.page + 1
                    };
                } else {
                    self.col += 1;
                }
            }
            Addressing::Vertical => {
                if self.page >= self.page_hi {
                    self.page = self.page_lo;
                    self.col = if self.col >= self.col_hi {
                        self.col_lo
                    } else {
                        self.col + 1
                    };
                } else {
                    self.page += 1;
                }
            }
        }
    }

    /// The lit state of panel pixel `(x, y)` after remap, inverse and on/off.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        if !self.on {
            return false;
        }
        if self.all_on {
            return true;
        }
        let col = if self.seg_remap { COLS - 1 - x } else { x };
        let row = if self.com_flip { ROWS - 1 - y } else { y };
        let lit = self.ram[row / 8][col] >> (row % 8) & 1 != 0;
        lit != self.inverse
    }
}

impl BoardDevice for Ssd1306 {
    fn kind(&self) -> &'static str {
        "oled"
    }

    fn pins(&mut self, _t_ns: u64, levels: u8) {
        let before = self.pins;
        self.pins = levels;
        self.vbatc = levels & (1 << 6) != 0;
        self.vddc = levels & (1 << 7) != 0;
        // Reset is level-sensitive: held low, the controller is in reset.
        if levels & RES == 0 {
            if before & RES != 0 {
                self.reset();
                self.pins = levels;
            }
            return;
        }
        // CS rising or still high: the byte in progress is abandoned.
        if levels & CS != 0 {
            self.bits = 0;
            self.shift = 0;
            return;
        }
        // Sample on the rising edge of SCLK while selected.
        let rising = before & SCLK == 0 && levels & SCLK != 0;
        if rising {
            self.shift = (self.shift << 1) | u8::from(levels & SDIN != 0);
            self.bits += 1;
            if self.bits == 8 {
                let byte = self.shift;
                self.bits = 0;
                self.shift = 0;
                if levels & DC == 0 {
                    self.command(byte);
                } else {
                    self.data(byte);
                }
            }
        }
    }

    fn image(&self) -> Option<DeviceImage> {
        let mut rgba = Vec::with_capacity(COLS * ROWS * 4);
        // Brightness follows the contrast register a little, as a panel does.
        let lit = 140 + (u16::from(self.contrast) * 115 / 255) as u8;
        for y in 0..ROWS {
            for x in 0..COLS {
                if self.pixel(x, y) {
                    rgba.extend_from_slice(&[lit, lit, 255, 255]);
                } else {
                    rgba.extend_from_slice(&[6, 8, 14, 255]);
                }
            }
        }
        Some(DeviceImage {
            width: COLS as u32,
            height: ROWS as u32,
            rgba,
        })
    }

    fn describe(&self) -> String {
        format!(
            "SSD1306 128x32, {}, {} command(s), {} data byte(s), contrast {:#04x}",
            if self.on { "on" } else { "off" },
            self.commands,
            self.data_bytes,
            self.contrast
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font5x7;

    /// Clocks `bytes` into the device as SPI mode 0, `dc` selecting data.
    fn send(d: &mut Ssd1306, dc: bool, bytes: &[u8]) {
        let mut t = 0u64;
        let base = RES | if dc { DC } else { 0 };
        let mut put = |d: &mut Ssd1306, lv: u8| {
            t += 10;
            d.pins(t, lv);
        };
        put(d, base); // CS low
        for &b in bytes {
            for i in (0..8).rev() {
                let bit = if b >> i & 1 != 0 { SDIN } else { 0 };
                put(d, base | bit); // data set up, clock low
                put(d, base | bit | SCLK); // rising edge
                put(d, base | bit); // falling
            }
        }
        put(d, base | CS); // CS high
    }

    fn lit(d: &Ssd1306) -> usize {
        (0..ROWS)
            .flat_map(|y| (0..COLS).map(move |x| (x, y)))
            .filter(|&(x, y)| d.pixel(x, y))
            .count()
    }

    #[test]
    fn the_display_is_dark_until_switched_on() {
        let mut d = Ssd1306::new();
        send(&mut d, true, &[0xFF; 8]);
        assert_eq!(d.counters(), (0, 8));
        assert_eq!(lit(&d), 0, "off: nothing shows");
        send(&mut d, false, &[0xAF]);
        assert!(d.is_on());
        assert_eq!(lit(&d), 8 * 8, "the 8 bytes of the first page, 8 px each");
        send(&mut d, false, &[0xAE]);
        assert_eq!(lit(&d), 0);
    }

    #[test]
    fn page_addressing_places_bytes_by_page_and_column() {
        let mut d = Ssd1306::new();
        send(&mut d, false, &[0xAF, 0xB2, 0x03, 0x11]); // page 2, column 0x13
        send(&mut d, true, &[0x81, 0xC3]);
        assert_eq!(d.ram()[2][0x13], 0x81);
        assert_eq!(d.ram()[2][0x14], 0xC3);
        assert!(d.pixel(0x13, 16), "bit 0 of page 2 is row 16");
        assert!(d.pixel(0x13, 23), "bit 7 is row 23");
        assert!(!d.pixel(0x13, 17));
    }

    #[test]
    fn horizontal_addressing_wraps_through_the_window() {
        let mut d = Ssd1306::new();
        // Horizontal mode, columns 4..=5, pages 1..=2.
        send(&mut d, false, &[0x20, 0x00, 0x21, 4, 5, 0x22, 1, 2]);
        send(&mut d, true, &[1, 2, 3, 4]);
        assert_eq!(d.ram()[1][4], 1);
        assert_eq!(d.ram()[1][5], 2);
        assert_eq!(d.ram()[2][4], 3);
        assert_eq!(d.ram()[2][5], 4);
        send(&mut d, true, &[5]);
        assert_eq!(d.ram()[1][4], 5, "wrapped back to the window start");
    }

    #[test]
    fn vertical_addressing_walks_down_a_column_first() {
        let mut d = Ssd1306::new();
        send(&mut d, false, &[0x20, 0x01, 0x21, 0, 1, 0x22, 0, 3]);
        send(&mut d, true, &[10, 11, 12, 13, 14]);
        assert_eq!(
            [d.ram()[0][0], d.ram()[1][0], d.ram()[2][0], d.ram()[3][0]],
            [10, 11, 12, 13]
        );
        assert_eq!(d.ram()[0][1], 14);
    }

    #[test]
    fn inverse_remap_and_all_on_change_the_picture() {
        let mut d = Ssd1306::new();
        send(&mut d, false, &[0xAF]);
        send(&mut d, true, &[0x01]); // pixel (0, 0)
        assert!(d.pixel(0, 0));
        send(&mut d, false, &[0xA1]); // segment remap: column 0 is on the right
        assert!(d.pixel(127, 0) && !d.pixel(0, 0));
        send(&mut d, false, &[0xC8]); // COM flip: row 0 is at the bottom
        assert!(d.pixel(127, 31));
        send(&mut d, false, &[0xA7]);
        assert_eq!(lit(&d), 128 * 32 - 1, "inverse");
        send(&mut d, false, &[0xA5]);
        assert_eq!(lit(&d), 128 * 32, "entire display on");
    }

    #[test]
    fn reset_restores_defaults_but_keeps_the_ram() {
        let mut d = Ssd1306::new();
        send(&mut d, false, &[0xAF, 0xA7]);
        send(&mut d, true, &[0xFF]);
        d.pins(1, 0); // RES low (CS low too)
        d.pins(2, RES | CS);
        assert!(!d.is_on());
        assert_eq!(d.ram()[0][0], 0xFF);
        send(&mut d, false, &[0xAF]);
        assert!(d.pixel(0, 0), "not inverse any more");
    }

    #[test]
    fn nothing_is_taken_while_deselected_or_in_reset() {
        let mut d = Ssd1306::new();
        // CS high: clocks are ignored.
        for _ in 0..16 {
            d.pins(1, RES | CS | SCLK | SDIN | DC);
            d.pins(2, RES | CS | SDIN | DC);
        }
        assert_eq!(d.counters(), (0, 0));
        // A byte cut short by CS rising is abandoned.
        d.pins(3, RES | DC);
        for _ in 0..4 {
            d.pins(4, RES | DC | SDIN);
            d.pins(5, RES | DC | SDIN | SCLK);
        }
        d.pins(6, RES | CS | DC);
        send(&mut d, true, &[0x55]);
        assert_eq!(d.counters(), (0, 1), "only the complete byte counted");
        assert_eq!(d.ram()[0][0], 0x55);
    }

    #[test]
    fn text_in_the_shared_font_round_trips_through_the_ram() {
        let mut d = Ssd1306::new();
        send(&mut d, false, &[0xAF, 0x20, 0x00]);
        let mut cols = Vec::new();
        for c in b"HI" {
            cols.extend_from_slice(&font5x7::glyph(*c));
            cols.push(0);
        }
        send(&mut d, true, &cols);
        assert_eq!(&d.ram()[0][..5], &font5x7::glyph(b'H'));
        assert_eq!(&d.ram()[0][6..11], &font5x7::glyph(b'I'));
        let img = d.image().unwrap();
        assert_eq!((img.width, img.height), (128, 32));
        assert_eq!(img.rgba.len(), 128 * 32 * 4);
    }

    #[test]
    fn the_font_covers_its_table() {
        assert_eq!(font5x7::CHARS.len(), font5x7::GLYPHS.len());
        assert_eq!(font5x7::glyph(b'a'), font5x7::glyph(b'A'));
        assert_eq!(font5x7::glyph(b'~'), [0x7F, 0x41, 0x41, 0x41, 0x7F]);
    }
}
