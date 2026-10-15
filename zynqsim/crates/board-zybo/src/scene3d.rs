//! A 3D view of the board.
//!
//! The 2D layout is extruded into boxes (PCB, connectors, Pmods, LEDs,
//! switches, buttons), projected through an orbit camera and rasterised with
//! the same software renderer as the 2D view, so it runs and tests headless.
//! Boxes that stand for a control carry the [`Hit`] they belong to, which makes
//! picking a point-in-polygon test over the drawn faces.
//!
//! This is an illustration of the layout, not a model of the real board: the
//! heights are plausible round numbers, not measurements. Nothing here is
//! used by the simulation.

use crate::def::{BoardDef, LedColor, PmodDomain};
use crate::geom;
use crate::render::{BoardImage, Hit};
use crate::state::{BoardState, PinLevel};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// An orbit camera around the middle of the board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Rotation about the vertical axis, radians.
    pub yaw: f32,
    /// Tilt from a top-down view, radians; 0 is the 2D view.
    pub pitch: f32,
    /// Zoom factor on top of the automatic fit; 1 fits the whole board.
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Camera {
        Camera {
            yaw: -0.45,
            pitch: 0.95,
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Largest tilt: past this the board is seen edge-on.
    pub const MAX_PITCH: f32 = 1.35;

    /// Rotates by a pointer drag of `(dx, dy)` pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw = (self.yaw + dx * 0.008).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + dy * 0.006).clamp(0.0, Camera::MAX_PITCH);
    }

    /// Changes the zoom by a scroll of `lines`, keeping it in a usable range.
    pub fn zoom_by(&mut self, lines: f32) {
        self.zoom = (self.zoom * (1.0 + lines * 0.1)).clamp(0.4, 6.0);
    }
}

/// An axis-aligned box on the board, in layout units; `z` grows out of the
/// PCB's top surface (the PCB itself reaches below zero).
struct Cuboid {
    c: [f32; 2],
    w: f32,
    d: f32,
    z0: f32,
    z1: f32,
    color: [u8; 3],
    hit: Option<Hit>,
}

/// A drawn quadrilateral that remembers what it belongs to.
#[derive(Clone, Debug)]
struct PickFace {
    poly: [[f32; 2]; 4],
    hit: Hit,
}

/// A rendered view with the information needed to pick controls in it.
#[derive(Clone, Debug)]
pub struct Scene {
    /// The picture.
    pub image: BoardImage,
    faces: Vec<PickFace>,
}

impl Scene {
    /// The control under image pixel `(x, y)`, frontmost first.
    pub fn pick(&self, x: f32, y: f32) -> Option<Hit> {
        self.faces
            .iter()
            .rev()
            .find(|f| inside(&f.poly, x, y))
            .map(|f| f.hit)
    }

    /// Image-space centre of the first drawn face of `hit`, if it is visible.
    pub fn centre_of(&self, hit: Hit) -> Option<(f32, f32)> {
        let f = self.faces.iter().rev().find(|f| f.hit == hit)?;
        let n = f.poly.len() as f32;
        Some((
            f.poly.iter().map(|p| p[0]).sum::<f32>() / n,
            f.poly.iter().map(|p| p[1]).sum::<f32>() / n,
        ))
    }
}

/// Point-in-convex-quad (either winding).
fn inside(poly: &[[f32; 2]; 4], x: f32, y: f32) -> bool {
    let mut sign = 0i8;
    for i in 0..4 {
        let a = poly[i];
        let b = poly[(i + 1) % 4];
        let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let s = if cross > 0.0 {
            1
        } else if cross < 0.0 {
            -1
        } else {
            0
        };
        if s != 0 {
            if sign != 0 && s != sign {
                return false;
            }
            sign = s;
        }
    }
    true
}

fn connector_height(kind: &str) -> (f32, [u8; 3]) {
    match kind {
        "soc" => (12.0, [38, 38, 44]),
        "ddr" => (7.0, [30, 44, 70]),
        "qspi" => (5.0, [30, 44, 70]),
        "hdmi_rx" | "hdmi_tx" => (34.0, [60, 60, 70]),
        "power" => (26.0, [70, 40, 40]),
        _ => (30.0, [150, 154, 160]),
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

fn blend(off: [u8; 3], on: [u8; 3], f: f32) -> [u8; 3] {
    std::array::from_fn(|i| (f32::from(off[i]) + (f32::from(on[i]) - f32::from(off[i])) * f) as u8)
}

/// Extrudes the layout into boxes, in drawing order (PCB first).
fn build(def: &BoardDef, state: &BoardState) -> Vec<Cuboid> {
    let mut v = Vec::new();
    let (lw, lh) = (def.layout.width, def.layout.height);
    v.push(Cuboid {
        c: [lw / 2.0, lh / 2.0],
        w: lw - 12.0,
        d: lh - 12.0,
        z0: -8.0,
        z1: 0.0,
        color: [14, 94, 56],
        hit: None,
    });
    for (i, c) in def.connectors.iter().enumerate() {
        let (h, color) = connector_height(&c.kind);
        v.push(Cuboid {
            c: [c.x, c.y],
            w: c.w,
            d: c.h,
            z0: 0.0,
            z1: h,
            color,
            hit: Some(Hit::Connector(i)),
        });
    }
    for (i, p) in def.pmods.iter().enumerate() {
        let body = if p.domain == PmodDomain::Ps {
            [34, 30, 52]
        } else {
            [28, 28, 30]
        };
        v.push(Cuboid {
            c: [p.x, p.y],
            w: geom::PMOD_W,
            d: geom::PMOD_H,
            z0: 0.0,
            z1: 18.0,
            color: body,
            hit: Some(Hit::Pmod(i)),
        });
        for (j, level) in state.pmod_pins[i].iter().enumerate() {
            let col = (j % 4) as f32;
            let row = (j / 4) as f32;
            let color = match level {
                PinLevel::Low => [70, 70, 76],
                PinLevel::High => [70, 230, 100],
                PinLevel::Z => [200, 170, 70],
            };
            v.push(Cuboid {
                c: [p.x - 39.0 + col * 26.0, p.y - 10.0 + row * 20.0],
                w: 14.0,
                d: 14.0,
                z0: 18.0,
                z1: 22.0,
                color,
                hit: Some(Hit::Pmod(i)),
            });
        }
    }
    for (i, l) in def.leds.iter().enumerate() {
        let f = f32::from(state.leds[i]) / 255.0;
        v.push(Cuboid {
            c: [l.x, l.y],
            w: geom::LED_R * 1.8,
            d: geom::LED_R * 1.8,
            z0: 0.0,
            z1: 6.0,
            color: blend([34, 40, 36], led_rgb(l.color), f),
            hit: Some(Hit::Led(i)),
        });
    }
    for (i, l) in def.rgb_leds.iter().enumerate() {
        let d = state.rgb[i];
        let body: [u8; 3] = std::array::from_fn(|k| 34u8.saturating_add(d[k] / 2));
        v.push(Cuboid {
            c: [l.x, l.y],
            w: geom::RGB_R * 1.8,
            d: geom::RGB_R * 1.8,
            z0: 0.0,
            z1: 6.0,
            color: body,
            hit: Some(Hit::Rgb(i)),
        });
    }
    for (i, s) in def.switches.iter().enumerate() {
        v.push(Cuboid {
            c: [s.x, s.y],
            w: geom::SWITCH_W,
            d: geom::SWITCH_H,
            z0: 0.0,
            z1: 10.0,
            color: [20, 20, 22],
            hit: Some(Hit::Switch(i)),
        });
        let on = state.switches[i];
        v.push(Cuboid {
            c: [s.x, if on { s.y - 15.0 } else { s.y + 15.0 }],
            w: geom::SWITCH_W - 8.0,
            d: 28.0,
            z0: 10.0,
            z1: if on { 24.0 } else { 18.0 },
            color: if on { [245, 245, 245] } else { [150, 150, 156] },
            hit: Some(Hit::Switch(i)),
        });
    }
    for (i, b) in def.buttons.iter().enumerate() {
        v.push(Cuboid {
            c: [b.x, b.y],
            w: geom::BUTTON,
            d: geom::BUTTON,
            z0: 0.0,
            z1: 6.0,
            color: [20, 20, 22],
            hit: Some(Hit::Button(i)),
        });
        let down = state.buttons[i];
        v.push(Cuboid {
            c: [b.x, b.y],
            w: geom::BUTTON - 10.0,
            d: geom::BUTTON - 10.0,
            z0: 6.0,
            z1: if down { 9.0 } else { 15.0 },
            color: if down {
                [255, 210, 90]
            } else {
                [110, 112, 120]
            },
            hit: Some(Hit::Button(i)),
        });
    }
    v
}

/// View-space transform: x right, y down, `t` toward the camera.
struct View {
    cy: f32,
    sy: f32,
    cp: f32,
    sp: f32,
    lw: f32,
    lh: f32,
}

impl View {
    fn new(def: &BoardDef, cam: &Camera) -> View {
        View {
            cy: cam.yaw.cos(),
            sy: cam.yaw.sin(),
            cp: cam.pitch.cos(),
            sp: cam.pitch.sin(),
            lw: def.layout.width,
            lh: def.layout.height,
        }
    }

    /// World point (layout x, layout y, height) to `(vx, vy, toward)`.
    fn xf(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let (x, y) = (x - self.lw / 2.0, y - self.lh / 2.0);
        let x1 = x * self.cy - y * self.sy;
        let y1 = x * self.sy + y * self.cy;
        [x1, y1 * self.cp - z * self.sp, y1 * self.sp + z * self.cp]
    }

    /// A direction (no translation) in the same space.
    fn dir(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let x1 = x * self.cy - y * self.sy;
        let y1 = x * self.sy + y * self.cy;
        [x1, y1 * self.cp - z * self.sp, y1 * self.sp + z * self.cp]
    }
}

/// Distance of the eye from the board's middle, in layout units; sets how
/// strong the perspective is.
const EYE: f32 = 2600.0;

struct Face {
    pts: [[f32; 3]; 4],
    normal: [f32; 3],
}

fn faces_of(c: &Cuboid, v: &View) -> Vec<Face> {
    let (x0, x1) = (c.c[0] - c.w / 2.0, c.c[0] + c.w / 2.0);
    let (y0, y1) = (c.c[1] - c.d / 2.0, c.c[1] + c.d / 2.0);
    let (z0, z1) = (c.z0, c.z1);
    let p = |x, y, z| v.xf(x, y, z);
    vec![
        Face {
            pts: [p(x0, y0, z1), p(x1, y0, z1), p(x1, y1, z1), p(x0, y1, z1)],
            normal: v.dir(0.0, 0.0, 1.0),
        },
        Face {
            pts: [p(x0, y1, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y1, z1)],
            normal: v.dir(0.0, 1.0, 0.0),
        },
        Face {
            pts: [p(x0, y0, z0), p(x1, y0, z0), p(x1, y0, z1), p(x0, y0, z1)],
            normal: v.dir(0.0, -1.0, 0.0),
        },
        Face {
            pts: [p(x1, y0, z0), p(x1, y1, z0), p(x1, y1, z1), p(x1, y0, z1)],
            normal: v.dir(1.0, 0.0, 0.0),
        },
        Face {
            pts: [p(x0, y0, z0), p(x0, y1, z0), p(x0, y1, z1), p(x0, y0, z1)],
            normal: v.dir(-1.0, 0.0, 0.0),
        },
    ]
}

fn shade(color: [u8; 3], normal: [f32; 3]) -> Color {
    // Headlight from the upper left, slightly off the view axis.
    const L: [f32; 3] = [-0.35, -0.45, 0.82];
    let dot = normal[0] * L[0] + normal[1] * L[1] + normal[2] * L[2];
    let k = 0.42 + 0.62 * dot.max(0.0);
    let c = |v: u8| ((f32::from(v) * k).clamp(0.0, 255.0)) as u8;
    Color::from_rgba8(c(color[0]), c(color[1]), c(color[2]), 255)
}

/// Renders the board from `cam` into a `width` x `height` image.
pub fn render_3d(
    def: &BoardDef,
    state: &BoardState,
    cam: &Camera,
    width: u32,
    height: u32,
) -> Scene {
    let (width, height) = (width.clamp(16, 4096), height.clamp(16, 4096));
    let boxes = build(def, state);
    let view = View::new(def, cam);

    // Perspective-project a view-space point at unit scale.
    let proj = |p: [f32; 3]| -> [f32; 2] {
        let f = EYE / (EYE - p[2]);
        [p[0] * f, p[1] * f]
    };

    // Fit the whole board (its PCB slab and tallest parts) to the viewport.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for c in &boxes {
        for f in faces_of(c, &view) {
            for q in f.pts {
                let s = proj(q);
                for k in 0..2 {
                    lo[k] = lo[k].min(s[k]);
                    hi[k] = hi[k].max(s[k]);
                }
            }
        }
    }
    let (bw, bh) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let fit = (width as f32 * 0.94 / bw).min(height as f32 * 0.94 / bh) * cam.zoom;
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let to_px = |p: [f32; 3]| -> [f32; 2] {
        let s = proj(p);
        [
            (s[0] - mid[0]) * fit + width as f32 / 2.0,
            (s[1] - mid[1]) * fit + height as f32 / 2.0,
        ]
    };

    let mut pm = Pixmap::new(width, height).unwrap_or_else(|| unreachable!("size is clamped"));
    pm.fill(Color::from_rgba8(27, 31, 35, 255));

    // Far to near by the box's middle; the PCB (first) is always at the back.
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    let depth = |c: &Cuboid| -view.xf(c.c[0], c.c[1], (c.z0 + c.z1) / 2.0)[2];
    order[1..].sort_by(|&a, &b| {
        depth(&boxes[b])
            .partial_cmp(&depth(&boxes[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut picks = Vec::new();
    for &i in &order {
        let c = &boxes[i];
        let mut fs = faces_of(c, &view);
        // Back faces away, then the rest far to near.
        fs.retain(|f| f.normal[2] > 1e-4);
        fs.sort_by(|a, b| {
            let da: f32 = a.pts.iter().map(|p| p[2]).sum();
            let db: f32 = b.pts.iter().map(|p| p[2]).sum();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        for f in &fs {
            let q: Vec<[f32; 2]> = f.pts.iter().map(|&p| to_px(p)).collect();
            let mut pb = PathBuilder::new();
            pb.move_to(q[0][0], q[0][1]);
            for p in &q[1..] {
                pb.line_to(p[0], p[1]);
            }
            pb.close();
            let Some(path) = pb.finish() else { continue };
            let mut paint = Paint::default();
            paint.set_color(shade(c.color, f.normal));
            paint.anti_alias = true;
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let mut edge = Paint::default();
            edge.set_color(Color::from_rgba8(0, 0, 0, 90));
            edge.anti_alias = true;
            pm.stroke_path(
                &path,
                &edge,
                &Stroke {
                    width: 0.8,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
            if let Some(h) = c.hit {
                picks.push(PickFace {
                    poly: [q[0], q[1], q[2], q[3]],
                    hit: h,
                });
            }
        }
    }

    // Glow over lit LEDs: translucent discs in the LED's colour.
    let glow = |pm: &mut Pixmap, x: f32, y: f32, r: f32, rgb: [u8; 3], f: f32| {
        if f <= 0.0 {
            return;
        }
        let centre = to_px(view.xf(x, y, 7.0));
        for k in 1..=3u8 {
            let alpha = (f * 80.0 / f32::from(k)) as u8;
            let rr = (r + f32::from(k) * 6.0) * fit * (EYE / (EYE - 7.0));
            if let Some(p) = PathBuilder::from_circle(centre[0], centre[1], rr) {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(rgb[0], rgb[1], rgb[2], alpha));
                paint.anti_alias = true;
                pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    };
    for (i, l) in def.leds.iter().enumerate() {
        glow(
            &mut pm,
            l.x,
            l.y,
            geom::LED_R,
            led_rgb(l.color),
            f32::from(state.leds[i]) / 255.0,
        );
    }
    for (i, l) in def.rgb_leds.iter().enumerate() {
        let d = state.rgb[i];
        let f = f32::from(d[0].max(d[1]).max(d[2])) / 255.0;
        glow(&mut pm, l.x, l.y, geom::RGB_R, d, f);
    }

    Scene {
        image: BoardImage {
            width,
            height,
            rgba: pm.take(),
        },
        faces: picks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> (BoardDef, BoardState) {
        let def = BoardDef::zybo_z7_20();
        let st = BoardState::new(&def);
        (def, st)
    }

    #[test]
    fn rendering_is_deterministic_and_fills_the_image() {
        let (def, st) = board();
        let a = render_3d(&def, &st, &Camera::default(), 640, 420);
        let b = render_3d(&def, &st, &Camera::default(), 640, 420);
        assert_eq!(a.image, b.image);
        assert_eq!(a.image.rgba.len(), 640 * 420 * 4);
        let bg = [27u8, 31, 35, 255];
        let drawn = a.image.rgba.chunks_exact(4).filter(|p| *p != bg).count();
        assert!(drawn > 640 * 420 / 5, "only {drawn} pixels drawn");
    }

    #[test]
    fn the_camera_changes_the_picture() {
        let (def, st) = board();
        let base = render_3d(&def, &st, &Camera::default(), 400, 300);
        let mut cam = Camera::default();
        cam.orbit(60.0, -30.0);
        assert_ne!(base.image, render_3d(&def, &st, &cam, 400, 300).image);
        let mut zoomed = Camera::default();
        zoomed.zoom_by(5.0);
        assert!(zoomed.zoom > 1.0);
        assert_ne!(base.image, render_3d(&def, &st, &zoomed, 400, 300).image);
        // Pitch is limited so the board never goes edge-on.
        let mut steep = Camera::default();
        steep.orbit(0.0, 100_000.0);
        assert_eq!(steep.pitch, Camera::MAX_PITCH);
        steep.orbit(0.0, -100_000.0);
        assert_eq!(steep.pitch, 0.0);
    }

    #[test]
    fn controls_are_found_where_they_are_drawn() {
        let (def, st) = board();
        for cam in [
            Camera::default(),
            Camera {
                yaw: 0.0,
                pitch: 0.0,
                zoom: 1.0,
            },
            Camera {
                yaw: 1.2,
                pitch: 0.6,
                zoom: 1.0,
            },
        ] {
            let sc = render_3d(&def, &st, &cam, 900, 600);
            for i in 0..def.switches.len() {
                let (x, y) = sc.centre_of(Hit::Switch(i)).expect("switch is visible");
                assert_eq!(sc.pick(x, y), Some(Hit::Switch(i)), "{cam:?} switch {i}");
            }
            for i in 0..def.buttons.len() {
                let (x, y) = sc.centre_of(Hit::Button(i)).expect("button is visible");
                assert_eq!(sc.pick(x, y), Some(Hit::Button(i)), "{cam:?} button {i}");
            }
            for i in 0..def.leds.len() {
                let (x, y) = sc.centre_of(Hit::Led(i)).expect("led is visible");
                assert_eq!(sc.pick(x, y), Some(Hit::Led(i)), "{cam:?} led {i}");
            }
            // Empty background picks nothing.
            assert_eq!(sc.pick(1.0, 1.0), None);
        }
    }

    #[test]
    fn state_shows_in_the_picture() {
        let (def, mut st) = board();
        let cam = Camera::default();
        let off = render_3d(&def, &st, &cam, 800, 520);
        st.leds[2] = 255;
        let lit = render_3d(&def, &st, &cam, 800, 520);
        assert_ne!(off.image, lit.image, "a lit LED changes pixels");
        let (x, y) = lit.centre_of(Hit::Led(2)).unwrap();
        let at = |s: &Scene| {
            let i = (y as usize * s.image.width as usize + x as usize) * 4;
            [s.image.rgba[i], s.image.rgba[i + 1], s.image.rgba[i + 2]]
        };
        assert!(
            u32::from(at(&lit)[1]) > u32::from(at(&off)[1]) + 40,
            "green channel of the LED body rose: {:?} -> {:?}",
            at(&off),
            at(&lit)
        );
        st.leds[2] = 0;
        st.switches[0] = true;
        let sw = render_3d(&def, &st, &cam, 800, 520);
        assert_ne!(off.image, sw.image, "a switch lever changes pixels");
        st.switches[0] = false;
        st.buttons[1] = true;
        let btn = render_3d(&def, &st, &cam, 800, 520);
        assert_ne!(off.image, btn.image, "a pressed button changes pixels");
    }

    #[test]
    fn front_boxes_hide_those_behind_them() {
        // Top-down, a switch lever (drawn on top of its body) is what a click
        // on the switch's centre reaches; both belong to the same switch.
        let (def, st) = board();
        let cam = Camera {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
        };
        let sc = render_3d(&def, &st, &cam, 1000, 700);
        let (x, y) = sc.centre_of(Hit::Switch(0)).unwrap();
        assert_eq!(sc.pick(x, y), Some(Hit::Switch(0)));
    }
}
