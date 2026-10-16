//! The memory viewer: a go-to address (numbers or ELF symbols), hex rows with
//! an ASCII column, bytes changed since the last read highlighted, and buttons
//! that follow the program counter or the stack pointer.

use super::{App, MemView};
use eframe::egui::{
    self, Color32, ColorImage, FontId, RichText, TextFormat, TextureOptions, text::LayoutJob,
};

const CHANGED: Color32 = Color32::from_rgb(235, 160, 20);
const GRAY: Color32 = Color32::GRAY;
/// Bytes read for the hex view: 16 rows.
pub const HEX_BYTES: u32 = 256;
/// Bytes per hex row.
pub const ROW_BYTES: usize = 16;

/// The width of one hex cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// One byte per cell.
    Bits8,
    /// Little-endian halfword per cell.
    Bits16,
    /// Little-endian word per cell.
    Bits32,
}

impl Unit {
    /// Bytes in one cell.
    pub fn bytes(self) -> usize {
        match self {
            Unit::Bits8 => 1,
            Unit::Bits16 => 2,
            Unit::Bits32 => 4,
        }
    }

    /// The button label.
    pub fn label(self) -> &'static str {
        match self {
            Unit::Bits8 => "8-bit",
            Unit::Bits16 => "16-bit",
            Unit::Bits32 => "32-bit",
        }
    }
}

/// A number as typed: `0x` hex or decimal.
pub fn parse_number(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// The address the go-to box names: a number, or `symbol` / `symbol+offset`
/// looked up with `lookup` (the simulation's symbol query).
pub fn resolve_address(
    text: &str,
    lookup: impl FnOnce(&str) -> Result<u64, String>,
) -> Result<u64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("enter an address or a symbol".to_string());
    }
    if let Some(v) = parse_number(t) {
        return Ok(v);
    }
    let (name, offset) = match t.split_once('+') {
        Some((n, o)) => (
            n.trim(),
            parse_number(o).ok_or_else(|| format!("offset `{}` is not a number", o.trim()))?,
        ),
        None => (t, 0),
    };
    let base = lookup(name)?;
    base.checked_add(offset)
        .ok_or_else(|| "address overflows".to_string())
}

/// Which bytes differ from the previous read of the same address. A read of
/// another address, or of another length, marks nothing.
pub fn changed_bytes(prev: Option<(u64, &[u8])>, addr: u64, cur: &[u8]) -> Vec<bool> {
    match prev {
        Some((a, p)) if a == addr && p.len() == cur.len() => {
            cur.iter().zip(p).map(|(c, p)| c != p).collect()
        }
        _ => vec![false; cur.len()],
    }
}

/// A printable character for a byte, `.` otherwise.
pub fn ascii(b: u8) -> char {
    if b.is_ascii_graphic() || b == b' ' {
        char::from(b)
    } else {
        '.'
    }
}

/// The cells of one row: each cell's text and whether any of its bytes changed.
pub fn row_cells(bytes: &[u8], changed: &[bool], unit: Unit) -> Vec<(String, bool)> {
    let n = unit.bytes();
    bytes
        .chunks(n)
        .enumerate()
        .map(|(i, cell)| {
            let mut v = 0u64;
            for (k, b) in cell.iter().enumerate() {
                v |= u64::from(*b) << (8 * k);
            }
            let text = format!("{v:0width$x}", width = 2 * n);
            let ch = changed.iter().skip(i * n).take(cell.len()).any(|c| *c);
            (text, ch)
        })
        .collect()
}

/// The hex dump as text for the clipboard: one line per row, `address  cells`.
pub fn copy_text(addr: u64, bytes: &[u8], unit: Unit) -> String {
    bytes
        .chunks(ROW_BYTES)
        .enumerate()
        .map(|(r, row)| {
            let cells: Vec<String> = row_cells(row, &[], unit)
                .into_iter()
                .map(|(t, _)| t)
                .collect();
            format!("{:08x}  {}", addr + (r * ROW_BYTES) as u64, cells.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl App {
    /// The section's one-line summary while it is closed.
    pub(super) fn memory_summary(&self) -> String {
        if !self.model.can_read_memory() {
            return "needs the qemu engine".to_string();
        }
        format!("{}, {}", self.mem_addr.trim(), self.mem_unit.label())
    }

    /// The memory viewer section.
    pub(super) fn memory_panel(&mut self, ui: &mut egui::Ui) {
        if !self.model.can_read_memory() {
            ui.label(
                RichText::new(
                    "Not available: this engine has no guest memory access (capability \
                     memory_access). The demo engine has none; use the qemu engine.",
                )
                .color(GRAY)
                .small(),
            );
            return;
        }
        let mut read = self.mem_auto && self.frames % 15 == 0;
        ui.horizontal(|ui| {
            ui.label("go to");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.mem_addr)
                    .desired_width(150.0)
                    .hint_text("0x01100000 or symbol+0x10"),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Read").clicked() || enter {
                read = true;
            }
            ui.checkbox(&mut self.mem_auto, "live");
        });
        let regs = self.model.cpu_regs.iter().find_map(|r| r.clone());
        ui.horizontal(|ui| {
            for (label, idx) in [("follow PC", 15usize), ("follow SP", 13)] {
                match &regs {
                    Some(r) => {
                        if ui.small_button(label).clicked() {
                            self.mem_addr = format!("{:#010x}", r.r[idx]);
                            read = true;
                        }
                    }
                    None => {
                        ui.add_enabled(false, egui::Button::new(label).small())
                            .on_disabled_hover_text(
                                "the registers are read while the guest is stopped",
                            );
                    }
                }
            }
            for (label, addr) in [
                ("source image", "0x01000000"),
                ("result image", "0x01100000"),
            ] {
                if ui.small_button(label).clicked() {
                    self.mem_addr = addr.to_string();
                    self.mem_view = MemView::ImageWords;
                    read = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mem_view, MemView::Hex, "hex");
            ui.selectable_value(
                &mut self.mem_view,
                MemView::ImageWords,
                "image (1 px / word)",
            );
            ui.selectable_value(
                &mut self.mem_view,
                MemView::ImageBytes,
                "image (1 px / byte)",
            );
        });
        if self.mem_view == MemView::Hex {
            ui.horizontal(|ui| {
                ui.label("read as");
                for u in [Unit::Bits8, Unit::Bits16, Unit::Bits32] {
                    ui.selectable_value(&mut self.mem_unit, u, u.label());
                }
                let copy = ui.add_enabled(
                    self.mem_data.is_some(),
                    egui::Button::new("copy as hex").small(),
                );
                if copy.clicked() {
                    if let Some((addr, data)) = &self.mem_data {
                        ui.ctx().copy_text(copy_text(*addr, data, self.mem_unit));
                    }
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label("w");
                ui.add(egui::TextEdit::singleline(&mut self.mem_w).desired_width(40.0));
                ui.label("h");
                ui.add(egui::TextEdit::singleline(&mut self.mem_h).desired_width(40.0));
            });
        }
        if read {
            self.read_memory(ui.ctx());
        }
        if let Some(e) = &self.mem_error {
            ui.colored_label(Color32::from_rgb(240, 100, 90), e);
        }
        match self.mem_view {
            MemView::Hex => self.hex_view(ui),
            _ => {
                if let Some(tex) = &self.mem_tex {
                    let size = tex.size_vec2();
                    let scale = (260.0 / size.x.max(size.y)).max(1.0).floor();
                    ui.add(egui::Image::new((tex.id(), size * scale)));
                }
            }
        }
    }

    /// The hex rows with an ASCII column; changed bytes in a warning colour.
    fn hex_view(&self, ui: &mut egui::Ui) {
        let Some((addr, data)) = &self.mem_data else {
            ui.label(RichText::new("no data read yet").color(GRAY).small());
            return;
        };
        let font = FontId::monospace(12.0);
        let plain = TextFormat {
            font_id: font.clone(),
            color: ui.visuals().text_color(),
            ..Default::default()
        };
        let changed_fmt = TextFormat {
            color: CHANGED,
            ..plain.clone()
        };
        let addr_fmt = TextFormat {
            color: GRAY,
            ..plain.clone()
        };
        egui::ScrollArea::vertical()
            .id_salt("mem_hex")
            .max_height(260.0)
            .show(ui, |ui| {
                for (r, row) in data.chunks(ROW_BYTES).enumerate() {
                    let start = r * ROW_BYTES;
                    let changed = &self
                        .mem_changed
                        .get(start..start + row.len())
                        .unwrap_or(&[]);
                    let mut job = LayoutJob::default();
                    job.append(
                        &format!("{:08x}  ", addr + start as u64),
                        0.0,
                        addr_fmt.clone(),
                    );
                    for (text, ch) in row_cells(row, changed, self.mem_unit) {
                        let fmt = if ch {
                            changed_fmt.clone()
                        } else {
                            plain.clone()
                        };
                        job.append(&format!("{text} "), 0.0, fmt);
                    }
                    let pad = (ROW_BYTES - row.len()) * 3;
                    job.append(&" ".repeat(pad), 0.0, plain.clone());
                    let ascii_text: String = row.iter().map(|b| ascii(*b)).collect();
                    job.append(&format!(" |{ascii_text}|"), 0.0, addr_fmt.clone());
                    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend));
                }
            });
        if self.mem_changed.iter().any(|c| *c) {
            ui.label(
                RichText::new("orange: changed since the previous read")
                    .small()
                    .color(GRAY),
            );
        }
    }

    /// Reads the go-to address (or the image size) and stores the result.
    pub(super) fn read_memory(&mut self, ctx: &egui::Context) {
        self.mem_error = None;
        let addr = match resolve_address(&self.mem_addr, |name| self.model.lookup_symbol(name)) {
            Ok(a) => a,
            Err(e) => {
                self.mem_error = Some(e);
                return;
            }
        };
        match self.mem_view {
            MemView::Hex => match self.model.mem_read(addr, HEX_BYTES) {
                Ok(d) => {
                    self.mem_changed = changed_bytes(
                        self.mem_data.as_ref().map(|(a, p)| (*a, p.as_slice())),
                        addr,
                        &d,
                    );
                    self.mem_data = Some((addr, d));
                }
                Err(e) => self.mem_error = Some(e),
            },
            view => {
                let (Some(w), Some(h)) = (parse_number(&self.mem_w), parse_number(&self.mem_h))
                else {
                    self.mem_error = Some("width/height are not numbers".to_string());
                    return;
                };
                if w == 0 || h == 0 || w * h > 1 << 18 {
                    self.mem_error =
                        Some("image size out of range (max 262144 pixels)".to_string());
                    return;
                }
                let per = if view == MemView::ImageWords { 4 } else { 1 };
                match self.model.mem_read(addr, (w * h * per) as u32) {
                    Ok(d) => {
                        let gray: Vec<u8> = d.chunks_exact(per as usize).map(|px| px[0]).collect();
                        let img = ColorImage::from_gray([w as usize, h as usize], &gray);
                        match &mut self.mem_tex {
                            Some(t) => t.set(img, TextureOptions::NEAREST),
                            None => {
                                self.mem_tex =
                                    Some(ctx.load_texture("mem", img, TextureOptions::NEAREST));
                            }
                        }
                    }
                    Err(e) => self.mem_error = Some(e),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_as_hex_decimal_or_symbol() {
        assert_eq!(parse_number("0x01100000"), Some(0x0110_0000));
        assert_eq!(parse_number(" 256 "), Some(256));
        assert_eq!(parse_number("zz"), None);

        let lookup = |name: &str| match name {
            "main" => Ok(0x0010_0400),
            _ => Err(format!("no symbol {name}")),
        };
        assert_eq!(resolve_address("0x10", lookup), Ok(0x10));
        assert_eq!(resolve_address("main", lookup), Ok(0x0010_0400));
        assert_eq!(resolve_address("main+0x20", lookup), Ok(0x0010_0420));
        assert!(
            resolve_address("nosuch", lookup)
                .unwrap_err()
                .contains("nosuch")
        );
        assert!(
            resolve_address("main+xyz", lookup)
                .unwrap_err()
                .contains("offset")
        );
        assert!(resolve_address("  ", lookup).is_err());
    }

    #[test]
    fn only_a_re_read_of_the_same_range_marks_changes() {
        let old = [1u8, 2, 3, 4];
        let new = [1u8, 9, 3, 7];
        assert_eq!(
            changed_bytes(Some((0x100, &old)), 0x100, &new),
            vec![false, true, false, true]
        );
        assert_eq!(
            changed_bytes(Some((0x200, &old)), 0x100, &new),
            vec![false; 4],
            "another address marks nothing"
        );
        assert_eq!(changed_bytes(None, 0x100, &new), vec![false; 4]);
    }

    #[test]
    fn cells_are_little_endian_values_of_the_chosen_width() {
        let bytes = [0x78u8, 0x56, 0x34, 0x12, 0xab];
        let changed = [false, false, false, false, true];
        let c32 = row_cells(&bytes, &changed, Unit::Bits32);
        assert_eq!(c32[0], ("12345678".to_string(), false));
        assert_eq!(c32[1], ("000000ab".to_string(), true));
        let c16 = row_cells(&bytes, &changed, Unit::Bits16);
        assert_eq!(c16[0].0, "5678");
        assert_eq!(c16[2], ("00ab".to_string(), true));
        let c8 = row_cells(&bytes, &changed, Unit::Bits8);
        assert_eq!(c8.len(), 5);
        assert_eq!(c8[0].0, "78");
    }

    #[test]
    fn copied_hex_has_one_line_per_row_and_no_ascii() {
        let bytes: Vec<u8> = (0..20).collect();
        let text = copy_text(0x0100_0000, &bytes, Unit::Bits8);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("01000000  00 01 02"));
        assert!(lines[1].starts_with("01000010  10 11 12 13"));
        assert!(!text.contains('|'));
        assert_eq!(ascii(b'A'), 'A');
        assert_eq!(ascii(0), '.');
    }
}
