//! The 3D board view: drag to orbit, scroll to zoom, click the controls.

use super::App;
use eframe::egui::{self, Color32, ColorImage, Rect, Sense, TextureOptions, pos2};
use sim_api::board::{Camera, Hit, Scene, render_3d};

/// What the last 3D render was made from.
#[derive(Clone, PartialEq)]
pub(super) struct Rendered {
    state: sim_api::board::BoardState,
    camera: (u32, u32, u32),
    size: (u32, u32),
}

fn camera_key(c: &Camera) -> (u32, u32, u32) {
    (
        (c.yaw * 1000.0) as i32 as u32,
        (c.pitch * 1000.0) as i32 as u32,
        (c.zoom * 1000.0) as u32,
    )
}

impl App {
    pub(super) fn board_view_3d(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let avail = ui.available_size();
        let ppp = ctx.pixels_per_point();
        let size = (
            ((avail.x * ppp).round() as u32).clamp(64, 3000),
            ((avail.y * ppp).round() as u32).clamp(64, 3000),
        );
        let key = Rendered {
            state: self.model.state.clone(),
            camera: camera_key(&self.cam),
            size,
        };
        if self.drawn3d.as_ref() != Some(&key) || self.scene.is_none() {
            let scene = render_3d(
                &self.model.def,
                &self.model.state,
                &self.cam,
                size.0,
                size.1,
            );
            let img = ColorImage::from_rgba_unmultiplied(
                [scene.image.width as usize, scene.image.height as usize],
                &scene.image.rgba,
            );
            match &mut self.tex3d {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => self.tex3d = Some(ctx.load_texture("board3d", img, TextureOptions::LINEAR)),
            }
            self.scene = Some(scene);
            self.drawn3d = Some(key);
        }
        let (Some(tex), Some(scene)) = (&self.tex3d, &self.scene) else {
            return;
        };

        let (rect, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
        ui.painter_at(rect).image(
            tex.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // Pointer position in image pixels.
        let local = resp.hover_pos().map(|p| {
            (
                (p.x - rect.min.x) / rect.width() * size.0 as f32,
                (p.y - rect.min.y) / rect.height() * size.1 as f32,
            )
        });
        let hit: Option<Hit> = local.and_then(|(x, y)| scene.pick(x, y));

        let (pressed, released) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        if pressed && resp.hovered() {
            match hit {
                Some(Hit::Switch(i)) => self.model.toggle_switch(i),
                Some(Hit::Button(i)) => {
                    self.held_button = Some(i);
                    self.model.set_button(i, true);
                }
                // Anything else starts an orbit drag.
                _ => self.orbiting = true,
            }
        }
        if released {
            self.orbiting = false;
            if let Some(i) = self.held_button.take() {
                self.model.set_button(i, false);
            }
        }
        if self.orbiting && resp.dragged() {
            let d = resp.drag_delta();
            self.cam.orbit(d.x, d.y);
        }
        if resp.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.cam.zoom_by(scroll / 40.0);
            }
        }
        if matches!(hit, Some(Hit::Switch(_) | Hit::Button(_))) {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if self.orbiting {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if let Some(h) = hit {
            let text = self.tooltip(h);
            resp.on_hover_text_at_pointer(text);
        }
    }

    /// The 2D/3D selector and, for 3D, camera shortcuts.
    pub(super) fn view_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view_3d, false, "2D board");
            ui.selectable_value(&mut self.view_3d, true, "3D board");
            if self.view_3d {
                ui.separator();
                if ui.small_button("reset view").clicked() {
                    self.cam = Camera::default();
                }
                if ui.small_button("top-down").clicked() {
                    self.cam = Camera {
                        yaw: 0.0,
                        pitch: 0.0,
                        zoom: 1.0,
                    };
                }
                ui.label(
                    egui::RichText::new("drag: rotate, wheel: zoom, click switches and buttons")
                        .small()
                        .color(Color32::GRAY),
                );
            }
        });
    }
}

/// Used by `App` to hold the cached 3D picture.
pub(super) type SceneCache = Option<Scene>;
