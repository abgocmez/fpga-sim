use crate::def::{BoardDef, LedColor};
use crate::geom;
use crate::state::{BoardState, PinLevel};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

/// A rendered frame: tightly packed RGBA, 8 bits per channel, row-major.
///
/// The board is fully opaque, so premultiplied and straight alpha coincide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// What lies under a point in layout space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Slide switch by index.
    Switch(usize),
    /// Push button by index.
    Button(usize),
    /// User LED by index.
    Led(usize),
    /// RGB LED by index.
    Rgb(usize),
    /// Pmod port by index.
    Pmod(usize),
    /// Connector or component by index.
    Connector(usize),
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn rect_path(cx: f32, cy: f32, w: f32, h: f32, radius: f32) -> Option<Path> {
    let (l, t, r, b) = (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
    let k = radius.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(l + k, t);
    pb.line_to(r - k, t);
    pb.quad_to(r, t, r, t + k);
    pb.line_to(r, b - k);
    pb.quad_to(r, b, r - k, b);
    pb.line_to(l + k, b);
    pb.quad_to(l, b, l, b - k);
    pb.line_to(l, t + k);
    pb.quad_to(l, t, l + k, t);
    pb.close();
    pb.finish()
}

struct Canvas {
    pm: Pixmap,
    tf: Transform,
}

impl Canvas {
    fn fill_rect(&mut self, cx: f32, cy: f32, w: f32, h: f32, radius: f32, c: Color) {
        if let Some(p) = rect_path(cx, cy, w, h, radius) {
            self.pm
                .fill_path(&p, &paint(c), FillRule::Winding, self.tf, None);
        }
    }

    fn stroke_rect(&mut self, cx: f32, cy: f32, w: f32, h: f32, radius: f32, c: Color) {
        if let Some(p) = rect_path(cx, cy, w, h, radius) {
            let stroke = Stroke {
                width: 2.0,
                ..Stroke::default()
            };
            self.pm.stroke_path(&p, &paint(c), &stroke, self.tf, None);
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm
                .fill_path(&p, &paint(c), FillRule::Winding, self.tf, None);
        }
    }
}

fn led_rgb(color: LedColor) -> [u8; 3] {
    match color {
        LedColor::Green => [60, 255, 90],
        LedColor::Red => [255, 60, 60],
        LedColor::Yellow => [255, 220, 60],
        LedColor::Blue => [70, 140, 255],
    }
}

/// Draws a lit LED: an off-state body blended towards `lit` by `duty`, plus a
/// halo whose strength follows the duty cycle.
fn draw_led(cv: &mut Canvas, cx: f32, cy: f32, r: f32, lit: [u8; 3], duty: [u8; 3], mono: bool) {
    let off = [34.0_f32, 40.0, 36.0];
    let (body, f) = if mono {
        let f = f32::from(duty[0]) / 255.0;
        let body: [f32; 3] = std::array::from_fn(|i| off[i] + (f32::from(lit[i]) - off[i]) * f);
        (body, f)
    } else {
        // RGB LED: each channel is lit independently by its own duty.
        let ch: [f32; 3] = std::array::from_fn(|i| f32::from(duty[i]));
        let body: [f32; 3] = std::array::from_fn(|i| off[i] + (ch[i] - off[i]).max(0.0));
        (body, ch.iter().fold(0.0_f32, |a, &b| a.max(b)) / 255.0)
    };
    if f > 0.0 {
        for k in 1..=3u8 {
            let alpha = (f * 70.0 / f32::from(k)) as u8;
            let halo = Color::from_rgba8(body[0] as u8, body[1] as u8, body[2] as u8, alpha);
            cv.fill_circle(cx, cy, r + f32::from(k) * 5.0, halo);
        }
    }
    cv.fill_circle(cx, cy, r + 2.0, rgb(15, 15, 15));
    cv.fill_circle(cx, cy, r, rgb(body[0] as u8, body[1] as u8, body[2] as u8));
}

fn pin_colour(l: PinLevel) -> Color {
    match l {
        PinLevel::Low => rgb(70, 70, 76),
        PinLevel::High => rgb(70, 230, 100),
        PinLevel::Z => rgb(200, 170, 70),
    }
}

fn connector_colour(kind: &str) -> (Color, Color) {
    match kind {
        "soc" => (rgb(38, 38, 44), rgb(90, 90, 100)),
        "ddr" | "qspi" => (rgb(30, 44, 70), rgb(80, 100, 140)),
        "hdmi_rx" | "hdmi_tx" => (rgb(60, 60, 70), rgb(160, 160, 175)),
        "power" => (rgb(70, 40, 40), rgb(170, 110, 110)),
        _ => (rgb(150, 154, 160), rgb(220, 224, 230)),
    }
}

/// Renders the board at `scale` pixels per layout unit.
///
/// Text labels are intentionally not rasterised here (the software renderer has
/// no font); front ends overlay labels from the [`BoardDef`] if they want them.
pub fn render(def: &BoardDef, state: &BoardState, scale: f32) -> BoardImage {
    let scale = scale.clamp(0.25, 4.0);
    let w = (def.layout.width * scale).round().max(1.0) as u32;
    let h = (def.layout.height * scale).round().max(1.0) as u32;
    let pm = Pixmap::new(w, h).unwrap_or_else(|| unreachable!("size is clamped to be non-zero"));
    let mut cv = Canvas {
        pm,
        tf: Transform::from_scale(scale, scale),
    };
    cv.pm.fill(rgb(27, 31, 35));

    // PCB.
    let (lw, lh) = (def.layout.width, def.layout.height);
    cv.fill_rect(
        lw / 2.0,
        lh / 2.0,
        lw - 12.0,
        lh - 12.0,
        18.0,
        rgb(14, 94, 56),
    );
    cv.stroke_rect(
        lw / 2.0,
        lh / 2.0,
        lw - 12.0,
        lh - 12.0,
        18.0,
        rgb(10, 66, 40),
    );

    // Connectors and major parts.
    for c in &def.connectors {
        let (fill, edge) = connector_colour(&c.kind);
        cv.fill_rect(c.x, c.y, c.w, c.h, 6.0, fill);
        cv.stroke_rect(c.x, c.y, c.w, c.h, 6.0, edge);
    }

    // Pmod ports: body plus two rows of four signal pins.
    for (i, p) in def.pmods.iter().enumerate() {
        let body = if p.domain == crate::def::PmodDomain::Ps {
            rgb(34, 30, 52)
        } else {
            rgb(28, 28, 30)
        };
        cv.fill_rect(p.x, p.y, geom::PMOD_W, geom::PMOD_H, 4.0, body);
        for (j, level) in state.pmod_pins[i].iter().enumerate() {
            let col = (j % 4) as f32;
            let row = (j / 4) as f32;
            let px = p.x - 39.0 + col * 26.0;
            let py = p.y - 10.0 + row * 20.0;
            cv.fill_rect(px, py, 14.0, 14.0, 2.0, pin_colour(*level));
        }
    }

    // User LEDs.
    for (i, l) in def.leds.iter().enumerate() {
        let d = state.leds[i];
        draw_led(
            &mut cv,
            l.x,
            l.y,
            geom::LED_R,
            led_rgb(l.color),
            [d, d, d],
            true,
        );
    }
    // RGB LEDs.
    for (i, l) in def.rgb_leds.iter().enumerate() {
        draw_led(
            &mut cv,
            l.x,
            l.y,
            geom::RGB_R,
            [255; 3],
            state.rgb[i],
            false,
        );
    }

    // Slide switches: knob up = on.
    for (i, s) in def.switches.iter().enumerate() {
        cv.fill_rect(
            s.x,
            s.y,
            geom::SWITCH_W,
            geom::SWITCH_H,
            6.0,
            rgb(20, 20, 22),
        );
        let on = state.switches[i];
        let ky = if on { s.y - 15.0 } else { s.y + 15.0 };
        let knob = if on {
            rgb(245, 245, 245)
        } else {
            rgb(150, 150, 156)
        };
        cv.fill_rect(s.x, ky, geom::SWITCH_W - 8.0, 28.0, 4.0, knob);
    }

    // Push buttons: pressed buttons are drawn sunken and bright.
    for (i, b) in def.buttons.iter().enumerate() {
        cv.fill_rect(b.x, b.y, geom::BUTTON, geom::BUTTON, 6.0, rgb(20, 20, 22));
        let (inset, col) = if state.buttons[i] {
            (14.0, rgb(255, 210, 90))
        } else {
            (8.0, rgb(110, 112, 120))
        };
        cv.fill_rect(
            b.x,
            b.y,
            geom::BUTTON - inset,
            geom::BUTTON - inset,
            4.0,
            col,
        );
    }

    BoardImage {
        width: w,
        height: h,
        rgba: cv.pm.take(),
    }
}

/// Renders the board and encodes it as PNG.
pub fn render_png(def: &BoardDef, state: &BoardState, scale: f32) -> Result<Vec<u8>, String> {
    let img = render(def, state, scale);
    encode_rgba_png(img.width, img.height, img.rgba)
}

/// Decodes a PNG into `(width, height, rgb)`. Transparent pixels are
/// composited over black.
pub fn decode_png_rgb(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let pm = Pixmap::decode_png(bytes).map_err(|e| e.to_string())?;
    let rgb: Vec<u8> = pm
        .data()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]]) // premultiplied: already over black
        .collect();
    Ok((pm.width(), pm.height(), rgb))
}

/// Encodes tightly packed, **opaque** RGBA pixels as PNG.
///
/// The buffer is treated as premultiplied; for fully opaque images, which is
/// all this workspace produces, that is identical to straight alpha.
pub fn encode_rgba_png(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let size = tiny_skia::IntSize::from_wh(width, height)
        .ok_or_else(|| "image has a zero dimension".to_string())?;
    let pm = Pixmap::from_vec(rgba, size)
        .ok_or_else(|| "pixel buffer does not match the image size".to_string())?;
    pm.encode_png().map_err(|e| e.to_string())
}

/// Finds the control under layout-space point `(x, y)`, topmost first:
/// switches, buttons, LEDs, RGB LEDs, Pmods, then connectors.
pub fn hit_test(def: &BoardDef, x: f32, y: f32) -> Option<Hit> {
    let in_rect =
        |cx: f32, cy: f32, w: f32, h: f32| (x - cx).abs() <= w / 2.0 && (y - cy).abs() <= h / 2.0;
    let in_circle = |cx: f32, cy: f32, r: f32| (x - cx).powi(2) + (y - cy).powi(2) <= r * r;

    if let Some(i) = def
        .switches
        .iter()
        .position(|s| in_rect(s.x, s.y, geom::SWITCH_W, geom::SWITCH_H))
    {
        return Some(Hit::Switch(i));
    }
    if let Some(i) = def
        .buttons
        .iter()
        .position(|b| in_rect(b.x, b.y, geom::BUTTON, geom::BUTTON))
    {
        return Some(Hit::Button(i));
    }
    if let Some(i) = def
        .leds
        .iter()
        .position(|l| in_circle(l.x, l.y, geom::LED_R + 2.0))
    {
        return Some(Hit::Led(i));
    }
    if let Some(i) = def
        .rgb_leds
        .iter()
        .position(|l| in_circle(l.x, l.y, geom::RGB_R + 2.0))
    {
        return Some(Hit::Rgb(i));
    }
    if let Some(i) = def
        .pmods
        .iter()
        .position(|p| in_rect(p.x, p.y, geom::PMOD_W, geom::PMOD_H))
    {
        return Some(Hit::Pmod(i));
    }
    def.connectors
        .iter()
        .position(|c| in_rect(c.x, c.y, c.w, c.h))
        .map(Hit::Connector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn golden_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("golden")
            .join(name)
    }

    /// Compares `img` with the stored golden PNG, tolerating +-2 per channel so
    /// anti-aliasing differences between CPUs do not fail the build. Set
    /// `UPDATE_GOLDEN=1` to (re)write the golden file.
    fn assert_golden(name: &str, def: &BoardDef, state: &BoardState) {
        let img = render(def, state, 1.0);
        let path = golden_path(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, render_png(def, state, 1.0).unwrap()).unwrap();
        }
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("missing golden {} ({e}); run with UPDATE_GOLDEN=1", name));
        let gold = Pixmap::decode_png(&bytes).unwrap();
        assert_eq!((gold.width(), gold.height()), (img.width, img.height));
        let worst = gold
            .data()
            .iter()
            .zip(&img.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 2, "{name}: max channel difference {worst} > 2");
    }

    #[test]
    fn default_board_matches_golden() {
        let def = BoardDef::zybo_z7_20();
        assert_golden("zybo_default.png", &def, &BoardState::new(&def));
    }

    #[test]
    fn active_board_matches_golden() {
        let def = BoardDef::zybo_z7_20();
        let mut s = BoardState::new(&def);
        s.switches[1] = true;
        s.switches[3] = true;
        s.buttons[2] = true;
        s.leds[0] = 255;
        s.leds[2] = 90;
        s.rgb[0] = [255, 0, 128];
        s.rgb[1] = [0, 200, 255];
        s.pmod_pins[0] = [
            PinLevel::High,
            PinLevel::Low,
            PinLevel::Z,
            PinLevel::High,
            PinLevel::Low,
            PinLevel::Low,
            PinLevel::High,
            PinLevel::Z,
        ];
        assert_golden("zybo_active.png", &def, &s);
    }

    #[test]
    fn state_changes_change_pixels_and_render_is_deterministic() {
        let def = BoardDef::zybo_z7_20();
        let mut s = BoardState::new(&def);
        let a = render(&def, &s, 1.0);
        assert_eq!(a, render(&def, &s, 1.0));
        s.leds[1] = 255;
        assert_ne!(a, render(&def, &s, 1.0));
    }

    #[test]
    fn scale_changes_image_size() {
        let def = BoardDef::zybo_z7_20();
        let img = render(&def, &BoardState::new(&def), 0.5);
        assert_eq!((img.width, img.height), (500, 320));
        assert_eq!(img.rgba.len(), 500 * 320 * 4);
    }

    #[test]
    fn png_has_signature() {
        let def = BoardDef::zybo_z7_20();
        let png = render_png(&def, &BoardState::new(&def), 0.5).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn hit_testing_finds_controls() {
        let def = BoardDef::zybo_z7_20();
        let sw = &def.switches[2];
        assert_eq!(hit_test(&def, sw.x, sw.y), Some(Hit::Switch(2)));
        let b = &def.buttons[3];
        assert_eq!(hit_test(&def, b.x + 5.0, b.y - 5.0), Some(Hit::Button(3)));
        let l = &def.leds[1];
        assert_eq!(hit_test(&def, l.x, l.y), Some(Hit::Led(1)));
        let p = &def.pmods[4];
        assert_eq!(hit_test(&def, p.x, p.y), Some(Hit::Pmod(4)));
        let soc = def.connectors.iter().position(|c| c.kind == "soc").unwrap();
        assert_eq!(
            hit_test(&def, def.connectors[soc].x, def.connectors[soc].y),
            Some(Hit::Connector(soc))
        );
        assert_eq!(hit_test(&def, 2.0, 2.0), None);
    }
}
