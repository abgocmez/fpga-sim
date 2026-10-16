//! The "Waveforms" tab: signal picker, time ruler, digital and bus traces,
//! cursor with value readout. The drawing is the shared scene of
//! `trace::wave_scene` (the MCP picture is drawn from the same scene); the
//! behaviour (zoom, pan, fetching) lives in `crate::waves`.

use super::{App, dialogs};
use crate::waves::{Group, elide_middle, group_of};
use eframe::egui::{
    self, Align2, Color32, FontId, PointerButton, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, pos2, vec2,
};
use sim_api::Ps;
use sim_api::trace::wave_scene::{
    BACKGROUND, Prim, SceneOpts, format_time, scene, tick_step, tone_rgb,
};

/// Width of the name and value column left of the plot.
const NAMES_W: f32 = 250.0;
/// Width of the value column inside the name column.
const VALUE_W: f32 = 74.0;
/// Height of the time ruler.
const HEADER_H: f32 = 22.0;
/// Height of one signal row.
const ROW_H: f32 = 30.0;
/// Least width, in points, of a right-drag selection that zooms.
const MIN_ZOOM_DRAG: f32 = 4.0;

/// Widget state of the tab that is not part of the headless model.
#[derive(Default)]
pub(super) struct WaveUi {
    /// The Waveforms tab is the one shown.
    pub tab: bool,
    /// Where the plot area was drawn last frame (screen points); used by
    /// tests to aim synthetic pointer input.
    pub plot: Option<Rect>,
    /// `(signal, value text)` as shown in the readout column last frame.
    pub readouts: Vec<(String, String)>,
    /// Where a right-button drag (zoom to selection) started, screen x.
    pub zoom_from: Option<f32>,
}

fn rgb(tone: sim_api::trace::wave_scene::Tone) -> Color32 {
    let c = tone_rgb(tone);
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The colour of a signal's name, by its group.
fn group_color(g: Group) -> Color32 {
    match g {
        Group::Board => Color32::from_rgb(120, 200, 255),
        Group::Uart => Color32::from_rgb(255, 190, 90),
        Group::Bus => Color32::from_rgb(150, 230, 150),
        Group::Pmod => Color32::from_rgb(240, 140, 220),
        Group::Other => Color32::from_rgb(200, 200, 200),
    }
}

impl App {
    /// The tab's content: picker on the left, plot on the right.
    pub(super) fn waves_view(&mut self, ui: &mut egui::Ui) {
        self.model.waves.active = true;
        egui::SidePanel::left("wave_picker")
            .resizable(true)
            .default_width(250.0)
            .show_inside(ui, |ui| self.wave_picker(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| self.wave_plot(ui));
    }

    fn wave_picker(&mut self, ui: &mut egui::Ui) {
        ui.strong("Signals");
        ui.add(
            egui::TextEdit::singleline(&mut self.model.waves.filter)
                .hint_text("search (words, any order)")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("select all matches")
                .on_hover_text("add every signal the search text matches")
                .clicked()
            {
                self.model.waves.select_matching();
            }
            if ui
                .small_button("none")
                .on_hover_text("hide every signal")
                .clicked()
            {
                self.model.waves.select_none();
            }
            if ui
                .small_button("defaults")
                .on_hover_text("replace the selection with the default signals")
                .clicked()
            {
                self.model.waves.pick_defaults();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("add group:").small());
            for g in Group::ALL {
                let n = self.model.waves.group_count(g);
                let button = egui::Button::new(format!("{} {n}", g.label())).small();
                if ui
                    .add_enabled(n > 0, button)
                    .on_hover_text("add every signal of this group")
                    .clicked()
                {
                    self.model.waves.select_group(g);
                }
            }
        });
        let n = self.model.waves.selected.len();
        ui.label(
            RichText::new(format!("{n} shown of {}", self.model.waves.catalogue.len())).small(),
        );
        ui.separator();
        if self.model.waves.catalogue.is_empty() {
            ui.label(
                RichText::new("The trace holds no signals yet: run the simulation.")
                    .color(Color32::GRAY)
                    .small(),
            );
            return;
        }
        let rows: Vec<(String, u8, usize, bool)> = self
            .model
            .waves
            .filtered()
            .iter()
            .map(|s| (s.name.clone(), s.width, s.changes, s.derived))
            .collect();
        if rows.is_empty() {
            ui.label(RichText::new("no signal matches").color(Color32::GRAY));
            return;
        }
        let h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        ScrollArea::vertical()
            .id_salt("wave_picker_list")
            .auto_shrink([false, false])
            .show_rows(ui, h, rows.len(), |ui, range| {
                for (name, width, changes, derived) in &rows[range] {
                    let mut on = self.model.waves.is_selected(name);
                    let label = if *width > 1 {
                        format!("{name} [{width}]")
                    } else {
                        name.clone()
                    };
                    let text =
                        RichText::new(elide_middle(&label, 34)).color(group_color(group_of(name)));
                    let r = ui.checkbox(&mut on, text).on_hover_text(format!(
                        "{label}\n{changes} change(s){}",
                        if *derived {
                            "; derived from bus accesses (not in the VCD)"
                        } else {
                            ""
                        }
                    ));
                    if r.changed() {
                        self.model.waves.toggle(name);
                    }
                }
            });
    }

    fn do_export_vcd(&mut self) {
        let dir = self.draft.dir.clone();
        let path = if dialogs::AVAILABLE {
            match dialogs::save_file("Export trace as VCD", &dir, "zynqsim-trace.vcd") {
                Some(p) => p,
                None => return,
            }
        } else {
            dir.join("zynqsim-trace.vcd")
        };
        let _ = self.model.export_vcd(&path);
    }

    fn wave_plot(&mut self, ui: &mut egui::Ui) {
        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.model.waves.follow, "follow live time");
            if ui
                .button("zoom to fit")
                .on_hover_text("show the whole run")
                .clicked()
            {
                self.model.waves.fit();
            }
            if ui.button("+").on_hover_text("zoom in").clicked() {
                self.model.waves.zoom_at(0.5, 2.0);
            }
            if ui.button("-").on_hover_text("zoom out").clicked() {
                self.model.waves.zoom_at(0.5, 0.5);
            }
            if ui
                .add_enabled(
                    self.model.waves.cursor_ps.is_some(),
                    egui::Button::new("clear cursor"),
                )
                .clicked()
            {
                self.model.waves.cursor_ps = None;
            }
            if ui
                .button("export VCD")
                .on_hover_text("write the whole trace (signals and UART bytes) as a VCD file")
                .clicked()
            {
                self.do_export_vcd();
            }
        });
        let w = &self.model.waves;
        ui.label(
            RichText::new(format!(
                "window {} .. {}   wheel: zoom   right drag: zoom to selection   drag: pan   click: cursor",
                Ps(w.from_ps),
                Ps(w.to_ps())
            ))
            .small()
            .color(Color32::GRAY),
        );
        if let Some(n) = self.model.waves.notice.clone() {
            ui.label(RichText::new(n).small());
        }
        if self.model.waves.selected.is_empty() {
            self.wave_ui.plot = None;
            self.wave_ui.readouts.clear();
            ui.add_space(12.0);
            ui.label(
                RichText::new(
                    "No signal is shown. Pick the defaults (LEDs, switches, buttons, UART, \
                     Pmod pins, a bus address) or tick signals on the left.",
                )
                .color(Color32::GRAY),
            );
            if ui.button("Pick default signals").clicked() {
                self.model.waves.pick_defaults();
            }
            self.model.sync_waves(false);
            return;
        }

        let n = self.model.waves.selected.len();
        let need_h = HEADER_H + ROW_H * n as f32;
        ScrollArea::vertical()
            .id_salt("wave_plot_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let avail_w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(vec2(avail_w, need_h), Sense::hover());
                let plot = Rect::from_min_max(
                    pos2((rect.min.x + NAMES_W).min(rect.max.x - 40.0), rect.min.y),
                    rect.max,
                );
                let resp = ui.interact(
                    plot,
                    ui.id().with("wave_plot_area"),
                    Sense::click_and_drag(),
                );
                self.wave_ui.plot = Some(plot);
                // Picoseconds from the left edge of the window to screen x,
                // with the window width as it is right now.
                let t_of = |span_ps: u64, x: f32| -> f64 {
                    f64::from((x - plot.min.x) / plot.width().max(1.0)) * span_ps.max(1) as f64
                };
                let mut changed = false;
                // Wheel: zoom around the pointer. Horizontal scroll pans.
                if resp.hovered() {
                    let (sy, sx) = ui.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x));
                    if let (Some(p), true) = (resp.hover_pos(), sy != 0.0) {
                        let anchor = f64::from((p.x - plot.min.x) / plot.width().max(1.0));
                        self.model
                            .waves
                            .zoom_at(anchor, (f64::from(sy) / 300.0).exp());
                        changed = true;
                    }
                    if sx != 0.0 {
                        let span = self.model.waves.span_ps;
                        self.model.waves.pan(-t_of(span, plot.min.x + sx));
                        changed = true;
                    }
                }
                // Drag with the left button: pan.
                if resp.dragged_by(PointerButton::Primary) {
                    let dx = resp.drag_delta().x;
                    if dx != 0.0 {
                        let span = self.model.waves.span_ps;
                        self.model.waves.pan(-t_of(span, plot.min.x + dx));
                        changed = true;
                    }
                }
                // Drag with the right button: zoom to the dragged range.
                if resp.drag_started_by(PointerButton::Secondary) {
                    self.wave_ui.zoom_from = ui.input(|i| i.pointer.press_origin()).map(|p| p.x);
                }
                if resp.drag_stopped_by(PointerButton::Secondary) {
                    let end = resp
                        .interact_pointer_pos()
                        .or_else(|| ui.input(|i| i.pointer.latest_pos()))
                        .map(|p| p.x);
                    if let (Some(a), Some(b)) = (self.wave_ui.zoom_from.take(), end) {
                        let lo = a.min(b).clamp(plot.min.x, plot.max.x);
                        let hi = a.max(b).clamp(plot.min.x, plot.max.x);
                        if hi - lo >= MIN_ZOOM_DRAG {
                            let w = &self.model.waves;
                            let t0 = w.from_ps as f64 + t_of(w.span_ps, lo);
                            let t1 = w.from_ps as f64 + t_of(w.span_ps, hi);
                            self.model
                                .waves
                                .zoom_to(t0.max(0.0) as u64, t1.max(0.0) as u64);
                            changed = true;
                        }
                    }
                }
                // Click: place the cursor.
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let w = &self.model.waves;
                        let t = w.from_ps as f64 + t_of(w.span_ps, p.x);
                        self.model.waves.cursor_ps = Some(t.max(0.0) as u64);
                    }
                }
                self.model.sync_waves(changed);

                // Readout time: the cursor, else the pointer, else the right edge.
                let hover_ps = resp.hover_pos().map(|p| {
                    let w = &self.model.waves;
                    (w.from_ps as f64 + t_of(w.span_ps, p.x)).max(0.0) as u64
                });

                let painter = ui.painter_at(rect);
                let bg = Color32::from_rgb(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2]);
                painter.rect_filled(rect, 0.0, bg);
                let mono = FontId::monospace(11.0);
                let char_w = ui.fonts(|f| f.glyph_width(&mono, '0'));

                let waves = &self.model.waves;
                let Some(data) = &waves.data else {
                    painter.text(
                        rect.min + vec2(8.0, 14.0),
                        Align2::LEFT_CENTER,
                        "loading...",
                        mono,
                        Color32::GRAY,
                    );
                    return;
                };
                let probe = waves.cursor_ps.or(hover_ps).unwrap_or(data.to_ps);
                let span = data.to_ps.saturating_sub(data.from_ps).max(1);
                let opts = SceneOpts {
                    plot_w: plot.width(),
                    label_w: 0.0,
                    row_h: ROW_H,
                    header_h: HEADER_H,
                    char_w,
                    cursor_ps: waves.cursor_ps,
                };
                let sc = scene(data, &opts);
                let o = plot.min;
                for p in &sc.prims {
                    match p {
                        Prim::Line {
                            x0,
                            y0,
                            x1,
                            y1,
                            tone,
                        } => {
                            painter.line_segment(
                                [o + vec2(*x0, *y0), o + vec2(*x1, *y1)],
                                Stroke::new(1.0_f32, rgb(*tone)),
                            );
                        }
                        Prim::Rect { x, y, w, h, tone } => {
                            painter.rect_filled(
                                Rect::from_min_size(o + vec2(*x, *y), vec2(*w, *h)),
                                0.0,
                                rgb(*tone),
                            );
                        }
                        Prim::Text { x, y, text, tone } => {
                            painter.text(
                                o + vec2(*x, *y),
                                Align2::LEFT_CENTER,
                                text,
                                mono.clone(),
                                rgb(*tone),
                            );
                        }
                    }
                }
                // The ruler's unit: the unit of its tick labels.
                let step = tick_step(span, plot.width(), 110.0);
                let unit = format_time(step, step)
                    .rsplit(' ')
                    .next()
                    .unwrap_or("ps")
                    .to_string();
                painter.text(
                    pos2(plot.max.x - 4.0, rect.min.y + HEADER_H / 2.0),
                    Align2::RIGHT_CENTER,
                    format!("time in {unit}"),
                    mono.clone(),
                    Color32::GRAY,
                );
                if let (Some(h), None) = (hover_ps, waves.cursor_ps) {
                    let x = plot.min.x
                        + ((h as f64 - data.from_ps as f64)
                            / (data.to_ps.saturating_sub(data.from_ps)).max(1) as f64
                            * f64::from(plot.width())) as f32;
                    painter.vline(
                        x,
                        plot.y_range(),
                        Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
                    );
                }
                // The range a right-button drag would zoom to.
                if let (Some(a), Some(b)) =
                    (self.wave_ui.zoom_from, ui.input(|i| i.pointer.latest_pos()))
                {
                    let r = Rect::from_x_y_ranges(
                        a.min(b.x).clamp(plot.min.x, plot.max.x)
                            ..=a.max(b.x).clamp(plot.min.x, plot.max.x),
                        plot.y_range(),
                    );
                    painter.rect(
                        r,
                        0.0,
                        Color32::from_white_alpha(24),
                        Stroke::new(1.0_f32, Color32::from_white_alpha(120)),
                        StrokeKind::Inside,
                    );
                }

                // Name column (coloured by group, elided, full name on hover)
                // and the value column at the probe time.
                let mut readouts = Vec::new();
                painter.text(
                    rect.min + vec2(4.0, HEADER_H / 2.0),
                    Align2::LEFT_CENTER,
                    format!("@ {}", Ps(probe)),
                    mono.clone(),
                    rgb(sim_api::trace::wave_scene::Tone::Cursor),
                );
                let name_w = NAMES_W - VALUE_W - 26.0;
                let name_chars = ((name_w - 4.0) / char_w).max(8.0) as usize;
                let mut remove: Option<String> = None;
                for (i, s) in data.signals.iter().enumerate() {
                    let y = rect.min.y + HEADER_H + i as f32 * ROW_H;
                    let row = Rect::from_min_size(pos2(rect.min.x, y), vec2(NAMES_W, ROW_H));
                    let value = waves.readout(s, probe.clamp(data.from_ps, data.to_ps));
                    painter.text(
                        row.min + vec2(4.0, ROW_H / 2.0),
                        Align2::LEFT_CENTER,
                        elide_middle(&s.name, name_chars),
                        mono.clone(),
                        group_color(group_of(&s.name)),
                    );
                    let name_rect = Rect::from_min_size(row.min, vec2(name_w, ROW_H));
                    ui.interact(name_rect, ui.id().with(("wave_name", i)), Sense::hover())
                        .on_hover_text(&s.name);
                    painter.text(
                        pos2(row.max.x - 26.0, row.min.y + ROW_H / 2.0),
                        Align2::RIGHT_CENTER,
                        &value,
                        mono.clone(),
                        rgb(sim_api::trace::wave_scene::Tone::BusText),
                    );
                    readouts.push((s.name.clone(), value));
                    let b = Rect::from_min_size(
                        pos2(row.max.x - 22.0, row.min.y + 5.0),
                        vec2(18.0, 18.0),
                    );
                    if ui
                        .put(b, egui::Button::new("x").small())
                        .on_hover_text("hide this signal")
                        .clicked()
                    {
                        remove = Some(s.name.clone());
                    }
                }
                painter.rect_stroke(
                    plot,
                    0.0,
                    Stroke::new(1.0_f32, rgb(sim_api::trace::wave_scene::Tone::Grid)),
                    StrokeKind::Inside,
                );
                self.wave_ui.readouts = readouts;
                if let Some(n) = remove {
                    self.model.waves.toggle(&n);
                }
            });
    }
}
