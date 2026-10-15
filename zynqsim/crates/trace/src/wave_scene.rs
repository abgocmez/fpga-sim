//! A drawing of a [`Waveform`] as a list of axis-aligned primitives.
//!
//! The same scene feeds the GUI (drawn with its painter), an SVG file and a
//! raster image (PNG through the front end's encoder), so what the viewer
//! shows and what an MCP client is sent cannot drift apart. There are no
//! diagonals: digital traces are steps and buses are boxes.

use crate::waveform::{SignalWave, Waveform};
use std::fmt::Write as _;

/// What a primitive means; back ends map tones to colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Time ruler ticks and row separators.
    Grid,
    /// Names and time labels.
    Label,
    /// A driven digital trace or bus outline.
    Wave,
    /// A digital trace that is high (drawn as a thin fill under the line).
    High,
    /// Text inside a bus box.
    BusText,
    /// A signal that has not been driven yet.
    Undriven,
    /// Changes closer together than a pixel.
    Activity,
    /// Data that is missing or cut short.
    Warn,
    /// The measurement cursor.
    Cursor,
}

/// One drawing primitive. Coordinates are in pixels from the top left.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// A straight line (horizontal or vertical).
    Line {
        /// Start x.
        x0: f32,
        /// Start y.
        y0: f32,
        /// End x.
        x1: f32,
        /// End y.
        y1: f32,
        /// Meaning.
        tone: Tone,
    },
    /// A filled rectangle.
    Rect {
        /// Left.
        x: f32,
        /// Top.
        y: f32,
        /// Width.
        w: f32,
        /// Height.
        h: f32,
        /// Meaning.
        tone: Tone,
    },
    /// Text, left aligned at `x`, vertically centred on `y`.
    Text {
        /// Left.
        x: f32,
        /// Centre line.
        y: f32,
        /// The text.
        text: String,
        /// Meaning.
        tone: Tone,
    },
}

/// Layout of a scene, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneOpts {
    /// Width of the plot area (the time axis).
    pub plot_w: f32,
    /// Width of the name column to the left of the plot; 0 leaves names to
    /// the caller (the GUI draws them as widgets).
    pub label_w: f32,
    /// Height of one signal row.
    pub row_h: f32,
    /// Height of the time ruler.
    pub header_h: f32,
    /// Width of one text character (monospace), used to decide what fits.
    pub char_w: f32,
    /// Where to draw the cursor, if it is inside the window.
    pub cursor_ps: Option<u64>,
}

impl Default for SceneOpts {
    fn default() -> SceneOpts {
        SceneOpts {
            plot_w: 800.0,
            label_w: 0.0,
            row_h: 28.0,
            header_h: 22.0,
            char_w: 7.0,
            cursor_ps: None,
        }
    }
}

/// A drawing, ready for a back end.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Total width, `label_w + plot_w`.
    pub width: f32,
    /// Total height, `header_h` plus the rows.
    pub height: f32,
    /// The primitives, back to front.
    pub prims: Vec<Prim>,
}

/// Formats a value of a `width`-bit signal: `0`/`1`, or hex padded to the
/// width.
pub fn format_value(value: u64, width: u8) -> String {
    if width <= 1 {
        format!("{}", value & 1)
    } else {
        let digits = usize::from(width.min(64)).div_ceil(4);
        format!("{value:0digits$x}")
    }
}

/// A time label for a tick; the unit is chosen from `step_ps`, the distance
/// between ticks, so every label shows the digits that tell ticks apart.
pub fn format_time(t_ps: u64, step_ps: u64) -> String {
    let (unit, name) = if step_ps >= 1_000_000_000_000 {
        (1_000_000_000_000u64, "s")
    } else if step_ps >= 1_000_000_000 {
        (1_000_000_000, "ms")
    } else if step_ps >= 1_000_000 {
        (1_000_000, "us")
    } else if step_ps >= 1_000 {
        (1_000, "ns")
    } else {
        (1, "ps")
    };
    let whole = t_ps / unit;
    let frac = t_ps % unit;
    if frac == 0 {
        return format!("{whole} {name}");
    }
    let digits = unit.ilog10() as usize;
    let mut f = format!("{frac:0digits$}");
    while f.ends_with('0') {
        f.pop();
    }
    format!("{whole}.{f} {name}")
}

/// The tick distance, a 1/2/5 times a power of ten, giving roughly
/// `target_px` pixels between ticks.
pub fn tick_step(span_ps: u64, plot_w: f32, target_px: f32) -> u64 {
    let per_px = span_ps as f64 / f64::from(plot_w.max(1.0));
    let want = (per_px * f64::from(target_px)).max(1.0);
    let mag = 10f64.powf(want.log10().floor());
    let m = want / mag;
    let nice = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    ((nice * mag) as u64).max(1)
}

/// One run of equal value drawn between two x positions.
struct Seg {
    x0: f32,
    x1: f32,
    value: Option<u64>,
    /// Several changes fell inside the first pixel of this run.
    busy: bool,
}

/// Runs of equal value from the left edge to `x_end`. Changes closer than a
/// pixel to the start of the open run are folded into it (and mark it
/// `busy`), so the result is bounded by the width, not by the changes.
fn segments(s: &SignalWave, x_of: &dyn Fn(u64) -> f32, x_left: f32, x_end: f32) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut x = x_left;
    let mut cur = s.initial;
    let mut busy = false;
    for &(t, v) in &s.changes {
        let cx = x_of(t);
        if cx - x < 1.0 {
            busy |= cx > x || !out.is_empty();
            cur = Some(v);
            continue;
        }
        out.push(Seg {
            x0: x,
            x1: cx,
            value: cur,
            busy,
        });
        x = cx;
        cur = Some(v);
        busy = false;
    }
    out.push(Seg {
        x0: x,
        x1: x_end,
        value: cur,
        busy,
    });
    out
}

fn row_prims(
    s: &SignalWave,
    y0: f32,
    o: &SceneOpts,
    x_of: &dyn Fn(u64) -> f32,
    x_left: f32,
    x_right: f32,
    prims: &mut Vec<Prim>,
) {
    let top = y0 + 4.0;
    let bot = y0 + o.row_h - 4.0;
    let mid = (top + bot) / 2.0;
    if !s.found {
        prims.push(Prim::Text {
            x: x_left + 4.0,
            y: mid,
            text: "not in the trace".to_string(),
            tone: Tone::Warn,
        });
        return;
    }
    let limit = if s.truncated {
        x_of(s.covered_to_ps).clamp(x_left, x_right)
    } else {
        x_right
    };
    let bus = s.width > 1;
    let segs = segments(s, x_of, x_left, limit);
    let mut prev: Option<(f32, Option<u64>)> = None;
    for g in &segs {
        let (x0, x1) = (g.x0.max(x_left), g.x1.min(limit));
        // A boundary: the vertical edge between the previous and this level.
        if let Some((_, pv)) = prev {
            if bus {
                if pv.is_some() || g.value.is_some() {
                    prims.push(Prim::Line {
                        x0,
                        y0: top,
                        x1: x0,
                        y1: bot,
                        tone: Tone::Wave,
                    });
                }
            } else {
                let y_of = |v: Option<u64>| match v {
                    Some(v) if v & 1 != 0 => top,
                    Some(_) => bot,
                    None => mid,
                };
                if pv.is_some() || g.value.is_some() {
                    prims.push(Prim::Line {
                        x0,
                        y0: y_of(pv),
                        x1: x0,
                        y1: y_of(g.value),
                        tone: Tone::Wave,
                    });
                }
            }
        }
        prev = Some((x0, g.value));
        if g.busy {
            prims.push(Prim::Rect {
                x: x0,
                y: top,
                w: 1.0,
                h: bot - top,
                tone: Tone::Activity,
            });
        }
        if x1 - x0 < 1.0 {
            // Many changes inside one pixel: show that something happens.
            if x1 >= x0 && g.value.is_some() {
                prims.push(Prim::Rect {
                    x: x0,
                    y: top,
                    w: 1.0,
                    h: bot - top,
                    tone: Tone::Activity,
                });
            }
            continue;
        }
        match g.value {
            None => prims.push(Prim::Line {
                x0,
                y0: mid,
                x1,
                y1: mid,
                tone: Tone::Undriven,
            }),
            Some(v) if !bus => {
                let y = if v & 1 != 0 { top } else { bot };
                prims.push(Prim::Line {
                    x0,
                    y0: y,
                    x1,
                    y1: y,
                    tone: Tone::Wave,
                });
                if v & 1 != 0 {
                    prims.push(Prim::Rect {
                        x: x0,
                        y: top,
                        w: x1 - x0,
                        h: 2.0,
                        tone: Tone::High,
                    });
                }
            }
            Some(v) => {
                for y in [top, bot] {
                    prims.push(Prim::Line {
                        x0,
                        y0: y,
                        x1,
                        y1: y,
                        tone: Tone::Wave,
                    });
                }
                let text = format_value(v, s.width);
                let need = (text.len() as f32 + 1.0) * o.char_w;
                if x1 - x0 >= need {
                    prims.push(Prim::Text {
                        x: x0 + o.char_w * 0.5,
                        y: mid,
                        text,
                        tone: Tone::BusText,
                    });
                }
            }
        }
    }
    if s.truncated && limit < x_right {
        prims.push(Prim::Rect {
            x: limit,
            y: top,
            w: x_right - limit,
            h: bot - top,
            tone: Tone::Warn,
        });
        let msg = "truncated: zoom in";
        if x_right - limit > msg.len() as f32 * o.char_w {
            prims.push(Prim::Text {
                x: limit + 4.0,
                y: mid,
                text: msg.to_string(),
                tone: Tone::Label,
            });
        }
    }
}

/// Lays out `w` as a scene: a time ruler on top and one row per signal.
pub fn scene(w: &Waveform, o: &SceneOpts) -> Scene {
    let span = w.to_ps.saturating_sub(w.from_ps).max(1);
    let x_left = o.label_w;
    let x_right = o.label_w + o.plot_w;
    let from = w.from_ps;
    let x_of = move |t: u64| {
        let f = t.saturating_sub(from) as f64 / span as f64;
        x_left + (f * f64::from(o.plot_w)) as f32
    };
    let height = o.header_h + o.row_h * w.signals.len() as f32;
    let mut prims = Vec::new();

    // Ruler.
    let step = tick_step(span, o.plot_w, 110.0);
    let mut t = w.from_ps.div_ceil(step) * step;
    while t <= w.to_ps {
        let x = x_of(t);
        prims.push(Prim::Line {
            x0: x,
            y0: o.header_h - 5.0,
            x1: x,
            y1: height,
            tone: Tone::Grid,
        });
        prims.push(Prim::Text {
            x: x + 3.0,
            y: o.header_h / 2.0 - 1.0,
            text: format_time(t, step),
            tone: Tone::Label,
        });
        match t.checked_add(step) {
            Some(n) => t = n,
            None => break,
        }
    }

    for (i, s) in w.signals.iter().enumerate() {
        let y0 = o.header_h + i as f32 * o.row_h;
        prims.push(Prim::Line {
            x0: 0.0,
            y0: y0 + o.row_h,
            x1: x_right,
            y1: y0 + o.row_h,
            tone: Tone::Grid,
        });
        if o.label_w > 0.0 {
            let name = if s.found {
                format!("{} [{}]", s.name, s.width)
            } else {
                s.name.clone()
            };
            let max = ((o.label_w - 6.0) / o.char_w).max(1.0) as usize;
            let name = if name.len() > max {
                format!("{}~", &name[..max.saturating_sub(1)])
            } else {
                name
            };
            prims.push(Prim::Text {
                x: 3.0,
                y: y0 + o.row_h / 2.0,
                text: name,
                tone: Tone::Label,
            });
        }
        row_prims(s, y0, o, &x_of, x_left, x_right, &mut prims);
    }

    if let Some(c) = o.cursor_ps {
        if c >= w.from_ps && c <= w.to_ps {
            let x = x_of(c);
            prims.push(Prim::Line {
                x0: x,
                y0: 0.0,
                x1: x,
                y1: height,
                tone: Tone::Cursor,
            });
        }
    }
    Scene {
        width: x_right,
        height,
        prims,
    }
}

/// RGB of a tone on the dark background used by [`to_svg`] and [`to_rgba`].
pub fn tone_rgb(t: Tone) -> [u8; 3] {
    match t {
        Tone::Grid => [60, 66, 78],
        Tone::Label => [200, 205, 215],
        Tone::Wave => [90, 220, 140],
        Tone::High => [60, 150, 100],
        Tone::BusText => [230, 235, 150],
        Tone::Undriven => [130, 90, 90],
        Tone::Activity => [240, 200, 80],
        Tone::Warn => [200, 120, 40],
        Tone::Cursor => [90, 170, 255],
    }
}

/// Background colour of the exported drawings.
pub const BACKGROUND: [u8; 3] = [24, 27, 33];

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The scene as a standalone SVG document.
pub fn to_svg(s: &Scene, char_w: f32) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{bg}\"/>\n",
        w = s.width,
        h = s.height,
        bg = hex_rgb(BACKGROUND),
    );
    for p in &s.prims {
        match p {
            Prim::Line {
                x0,
                y0,
                x1,
                y1,
                tone,
            } => {
                let _ = writeln!(
                    o,
                    "<line x1=\"{x0:.1}\" y1=\"{y0:.1}\" x2=\"{x1:.1}\" y2=\"{y1:.1}\" \
                     stroke=\"{}\" stroke-width=\"1\"/>",
                    hex_rgb(tone_rgb(*tone))
                );
            }
            Prim::Rect { x, y, w, h, tone } => {
                let _ = writeln!(
                    o,
                    "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" \
                     fill=\"{}\"/>",
                    hex_rgb(tone_rgb(*tone))
                );
            }
            Prim::Text { x, y, text, tone } => {
                let _ = writeln!(
                    o,
                    "<text x=\"{x:.1}\" y=\"{y:.1}\" font-family=\"monospace\" font-size=\"{:.0}\" \
                     dominant-baseline=\"central\" fill=\"{}\">{}</text>",
                    char_w / 0.6,
                    hex_rgb(tone_rgb(*tone)),
                    xml_escape(text)
                );
            }
        }
    }
    o.push_str("</svg>\n");
    o
}

fn hex_rgb(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// 3x5 glyph rows (3 bits each, MSB left) for the characters a waveform uses.
fn glyph(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 7, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '.' => [0, 0, 0, 0, 2],
        '_' => [0, 0, 0, 0, 7],
        '-' => [0, 0, 7, 0, 0],
        ':' => [0, 2, 0, 2, 0],
        '/' => [1, 1, 2, 4, 4],
        '=' => [0, 7, 0, 7, 0],
        '[' => [6, 4, 4, 4, 6],
        ']' => [3, 1, 1, 1, 3],
        '~' => [0, 3, 6, 0, 0],
        _ => [0, 0, 0, 0, 0],
    }
}

/// The scene as an opaque RGBA raster (`width * height * 4` bytes), drawn
/// with a built-in 3x5 font scaled by 2 (the advance is 8 pixels: build the
/// scene with `char_w = 8`). Returns `(width, height, rgba)`.
pub fn to_rgba(s: &Scene) -> (u32, u32, Vec<u8>) {
    let w = s.width.ceil().max(1.0) as usize;
    let h = s.height.ceil().max(1.0) as usize;
    let mut px = vec![0u8; w * h * 4];
    for p in px.chunks_exact_mut(4) {
        p[..3].copy_from_slice(&BACKGROUND);
        p[3] = 255;
    }
    let mut put = |x: i64, y: i64, c: [u8; 3]| {
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let i = (y as usize * w + x as usize) * 4;
            px[i..i + 3].copy_from_slice(&c);
        }
    };
    for p in &s.prims {
        match p {
            Prim::Line {
                x0,
                y0,
                x1,
                y1,
                tone,
            } => {
                let c = tone_rgb(*tone);
                let (xa, xb) = (x0.min(*x1).round() as i64, x0.max(*x1).round() as i64);
                let (ya, yb) = (y0.min(*y1).round() as i64, y0.max(*y1).round() as i64);
                for y in ya..=yb {
                    for x in xa..=xb {
                        put(x, y, c);
                    }
                }
            }
            Prim::Rect { x, y, w, h, tone } => {
                let c = tone_rgb(*tone);
                let (xa, ya) = (x.round() as i64, y.round() as i64);
                let (xb, yb) = (
                    (x + w).round().max(x.round() + 1.0) as i64,
                    (y + h).round().max(y.round() + 1.0) as i64,
                );
                for yy in ya..yb {
                    for xx in xa..xb {
                        put(xx, yy, c);
                    }
                }
            }
            Prim::Text { x, y, text, tone } => {
                let c = tone_rgb(*tone);
                let top = (y - 5.0).round() as i64;
                for (i, ch) in text.chars().enumerate() {
                    let gx = (x + i as f32 * 8.0).round() as i64;
                    for (row, bits) in glyph(ch).iter().enumerate() {
                        for col in 0..3 {
                            if bits >> (2 - col) & 1 != 0 {
                                for dy in 0..2 {
                                    for dx in 0..2 {
                                        put(gx + col * 2 + dx, top + row as i64 * 2 + dy, c);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (w as u32, h as u32, px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::SignalWave;

    fn wave(
        name: &str,
        width: u8,
        initial: Option<u64>,
        changes: &[(u64, u64)],
        to: u64,
    ) -> SignalWave {
        SignalWave {
            name: name.to_string(),
            width,
            found: true,
            initial,
            changes: changes.to_vec(),
            in_window: changes.len(),
            truncated: false,
            covered_to_ps: to,
        }
    }

    fn window(signals: Vec<SignalWave>, from: u64, to: u64) -> Waveform {
        Waveform {
            from_ps: from,
            to_ps: to,
            now_ps: to,
            trace_len: 0,
            dropped: 0,
            signals,
            signals_truncated: false,
            max_changes: 100,
        }
    }

    fn opts() -> SceneOpts {
        SceneOpts {
            plot_w: 1000.0,
            ..SceneOpts::default()
        }
    }

    #[test]
    fn values_and_times_are_formatted() {
        assert_eq!(format_value(1, 1), "1");
        assert_eq!(format_value(0xA5, 8), "a5");
        assert_eq!(format_value(0x5, 9), "005");
        assert_eq!(format_value(0xDEAD_BEEF, 32), "deadbeef");
        assert_eq!(format_time(0, 1_000_000), "0 us");
        assert_eq!(format_time(2_500_000, 500_000), "2500 ns");
        assert_eq!(format_time(2_500_000, 1_000_000), "2.5 us");
        assert_eq!(format_time(1_250_000_000, 250_000_000), "1250 us");
        assert_eq!(format_time(1_250_000_000, 1_000_000_000), "1.25 ms");
        assert_eq!(format_time(1_000_000_000_000, 500_000_000_000), "1000 ms");
        assert_eq!(format_time(3_000_000_000_000, 1_000_000_000_000), "3 s");
        assert_eq!(format_time(750, 250), "750 ps");
        assert_eq!(tick_step(1_000_000, 1000.0, 100.0), 100_000);
        assert_eq!(tick_step(3_000, 1000.0, 100.0), 500);
        assert_eq!(tick_step(0, 1000.0, 100.0), 1);
    }

    #[test]
    fn a_digital_trace_is_steps_at_the_right_pixels() {
        // Window 0..1000 ps over 1000 px: one pixel per picosecond.
        let w = window(
            vec![wave("a", 1, Some(0), &[(250, 1), (500, 0)], 1000)],
            0,
            1000,
        );
        let s = scene(&w, &opts());
        let edges: Vec<f32> = s
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line {
                    x0,
                    x1,
                    y0,
                    y1,
                    tone: Tone::Wave,
                } if x0 == x1 && y0 != y1 => Some(*x0),
                _ => None,
            })
            .collect();
        assert_eq!(edges, [250.0, 500.0]);
        // High between the edges: one High fill 250 px wide.
        let high: Vec<f32> = s
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Rect {
                    w,
                    tone: Tone::High,
                    ..
                } => Some(*w),
                _ => None,
            })
            .collect();
        assert_eq!(high, [250.0]);
        assert_eq!(s.height, opts().header_h + opts().row_h);
    }

    #[test]
    fn a_bus_shows_hex_when_it_fits_and_x_before_the_first_drive() {
        let w = window(
            vec![wave("d", 16, None, &[(100, 0xBEEF), (900, 0x1)], 1000)],
            0,
            1000,
        );
        let s = scene(&w, &opts());
        let texts: Vec<&str> = s
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Text {
                    text,
                    tone: Tone::BusText,
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // "beef" fits in its 800 px box; the final 0001 box (100 px) fits too.
        assert_eq!(texts, ["beef", "0001"]);
    }

    #[test]
    fn dense_changes_collapse_to_activity_and_zoom_reveals_them() {
        // 2000 changes inside 1 ms drawn over 100 px: many per pixel.
        let ch: Vec<(u64, u64)> = (0..2000u64).map(|i| (i * 500, i & 1)).collect();
        let w = window(vec![wave("clk", 1, Some(0), &ch, 1_000_000)], 0, 1_000_000);
        let o = SceneOpts {
            plot_w: 100.0,
            ..SceneOpts::default()
        };
        let dense = scene(&w, &o).prims.len();
        let act = scene(&w, &o)
            .prims
            .iter()
            .filter(|p| {
                matches!(
                    p,
                    Prim::Rect {
                        tone: Tone::Activity,
                        ..
                    }
                )
            })
            .count();
        assert!(act > 10, "activity marks: {act}");
        assert!(
            dense < 600,
            "bounded by the width, not the changes: {dense}"
        );
    }

    #[test]
    fn cursor_and_truncation_are_drawn_and_missing_signals_say_so() {
        let mut t = wave("a", 1, Some(0), &[(100, 1)], 1000);
        t.truncated = true;
        t.covered_to_ps = 500;
        let mut missing = wave("zz", 1, None, &[], 1000);
        missing.found = false;
        missing.width = 0;
        let w = window(vec![t, missing], 0, 1000);
        let s = scene(
            &w,
            &SceneOpts {
                cursor_ps: Some(400),
                label_w: 150.0,
                ..opts()
            },
        );
        assert!(s.prims.iter().any(
            |p| matches!(p, Prim::Line { x0, tone: Tone::Cursor, .. } if (*x0 - 550.0).abs() < 0.01)
        ));
        assert!(s.prims.iter().any(|p| matches!(
            p,
            Prim::Rect {
                tone: Tone::Warn,
                ..
            }
        )));
        assert!(s.prims.iter().any(
            |p| matches!(p, Prim::Text { text, tone: Tone::Warn, .. } if text.contains("not in"))
        ));
        assert!(
            s.prims
                .iter()
                .any(|p| matches!(p, Prim::Text { text, .. } if text == "a [1]"))
        );
        // A cursor outside the window is not drawn.
        let s = scene(
            &w,
            &SceneOpts {
                cursor_ps: Some(5000),
                ..opts()
            },
        );
        assert!(!s.prims.iter().any(|p| matches!(
            p,
            Prim::Line {
                tone: Tone::Cursor,
                ..
            }
        )));
    }

    #[test]
    fn svg_and_raster_back_ends_render_the_scene() {
        let w = window(
            vec![
                wave("board.led0", 1, Some(0), &[(300, 1)], 1000),
                wave("bus<&>", 8, Some(0x12), &[(500, 0xFE)], 1000),
            ],
            0,
            1000,
        );
        let o = SceneOpts {
            plot_w: 600.0,
            label_w: 160.0,
            char_w: 8.0,
            ..SceneOpts::default()
        };
        let sc = scene(&w, &o);
        let svg = to_svg(&sc, 8.0);
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("board.led0 [1]"));
        assert!(svg.contains("bus&lt;&amp;&gt;"), "names are escaped");
        let (pw, ph, rgba) = to_rgba(&sc);
        assert_eq!((pw, ph), (760, 22 + 56));
        assert_eq!(rgba.len(), (pw * ph * 4) as usize);
        let bg = &rgba[..4];
        assert_eq!(bg, [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
        // Something other than background was drawn, in the wave colour too.
        let wave_rgb = tone_rgb(Tone::Wave);
        assert!(rgba.chunks_exact(4).any(|p| p[..3] == wave_rgb));
        assert!(
            rgba.chunks_exact(4)
                .any(|p| p[..3] == tone_rgb(Tone::Label))
        );
    }
}
