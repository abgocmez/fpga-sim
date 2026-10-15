//! The egui front end: board view, control bar, console and info panels.

mod dialogs;
mod media;
mod project;
mod view3d;

use crate::loader::ProjectDraft;
use crate::model::{Model, SPEEDS};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Rect, RichText, Sense, TextureHandle,
    TextureOptions, pos2, vec2,
};
use sim_api::Ps;
use sim_api::board::{BoardState, Camera, Hit, hit_test, render};
use sim_api::trace::{TraceEvent, TraceRecord};
use std::time::{Duration, Instant};

/// Start-up options that exist for automated checks.
#[derive(Default)]
pub struct Opts {
    /// Close the window after this many milliseconds.
    pub exit_after: Option<Duration>,
    /// Save a PNG of the window here, then exit.
    pub screenshot: Option<std::path::PathBuf>,
    /// Open the memory viewer at this address as a 64x64 image.
    pub mem: Option<u64>,
    /// Open on the 3D board view.
    pub view_3d: bool,
    /// Open the project window.
    pub project_window: bool,
    /// Where relative paths of the project that was opened resolve, and the
    /// project file itself, for the project window.
    pub project: Option<std::path::PathBuf>,
}

/// How the memory viewer shows its data.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemView {
    Hex,
    ImageWords,
    ImageBytes,
}

/// The application state.
pub struct App {
    model: Model,
    opts: Opts,
    texture: Option<TextureHandle>,
    drawn: Option<(BoardState, u32)>,
    held_button: Option<usize>,
    key_down: Vec<bool>,
    console_tab: usize,
    console_input: String,
    crlf: bool,
    cpu_reason: [String; 2],
    pl_reason: String,
    started: Instant,
    frames: u32,
    screenshot_requested: bool,
    mem_addr: String,
    mem_w: String,
    mem_h: String,
    mem_view: MemView,
    mem_auto: bool,
    mem_text: String,
    mem_tex: Option<TextureHandle>,
    mem_error: Option<String>,
    mem_open: bool,
    // 3D view
    view_3d: bool,
    cam: Camera,
    tex3d: Option<TextureHandle>,
    drawn3d: Option<view3d::Rendered>,
    scene: view3d::SceneCache,
    /// Where the board picture was drawn last frame (screen points), and the
    /// scale from layout units to points in the 2D view. Used by tests to aim
    /// synthetic pointer input.
    board_rect: Option<Rect>,
    board_fit: f32,
    orbiting: bool,
    // project window
    project_open: bool,
    draft: ProjectDraft,
    draft_msg: Option<String>,
    open_requested: bool,
    autoplay: bool,
    seen_generation: u32,
    // media panels
    media: media::MediaUi,
}

impl App {
    /// Builds the app around a loaded model.
    pub fn new(model: Model, opts: Opts) -> App {
        let opts_mem = opts.mem;
        let cpu_reason = [
            model.cpu_unavailable_reason(0),
            model.cpu_unavailable_reason(1),
        ];
        let pl_reason = model.pl_unavailable_reason();
        let key_down = vec![false; model.def.buttons.len()];
        let draft = match &opts.project {
            Some(f) => ProjectDraft::load(f).unwrap_or_else(|_| ProjectDraft::demo()),
            None => ProjectDraft::demo(),
        };
        let view_3d = opts.view_3d;
        let opts_project_window = opts.project_window;
        App {
            model,
            opts,
            texture: None,
            drawn: None,
            held_button: None,
            key_down,
            console_tab: 1,
            console_input: String::new(),
            crlf: true,
            cpu_reason,
            pl_reason,
            started: Instant::now(),
            frames: 0,
            screenshot_requested: false,
            mem_addr: opts_mem.map_or_else(|| "0x01100000".to_string(), |a| format!("{a:#010x}")),
            mem_w: "64".to_string(),
            mem_h: "64".to_string(),
            mem_view: if opts_mem.is_some() {
                MemView::ImageWords
            } else {
                MemView::Hex
            },
            mem_auto: opts_mem.is_some(),
            mem_text: String::new(),
            mem_tex: None,
            mem_error: None,
            mem_open: opts_mem.is_some(),
            view_3d,
            cam: Camera::default(),
            tex3d: None,
            drawn3d: None,
            scene: None,
            board_rect: None,
            board_fit: 1.0,
            orbiting: false,
            project_open: opts_project_window,
            draft,
            draft_msg: None,
            open_requested: false,
            autoplay: false,
            seen_generation: 0,
            media: media::MediaUi::default(),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        // Switch shortcuts toggle on press; button shortcuts are held.
        let switch_keys: Vec<Option<Key>> = self
            .model
            .def
            .switches
            .iter()
            .map(|s| s.key.as_deref().and_then(key_from_label))
            .collect();
        for (i, k) in switch_keys.iter().enumerate() {
            if let Some(k) = k {
                if ctx.input(|inp| inp.key_pressed(*k)) {
                    self.model.toggle_switch(i);
                }
            }
        }
        let button_keys: Vec<Option<Key>> = self
            .model
            .def
            .buttons
            .iter()
            .map(|b| b.key.as_deref().and_then(key_from_label))
            .collect();
        for (i, k) in button_keys.iter().enumerate() {
            if let Some(k) = k {
                let down = ctx.input(|inp| inp.key_down(*k));
                if down != self.key_down[i] {
                    self.key_down[i] = down;
                    self.model.set_button(i, down);
                }
            }
        }
    }

    fn control_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Open project…")
                .on_hover_text("Load a project.toml: PS firmware, PL design, media")
                .clicked()
            {
                self.open_requested = true;
            }
            if ui
                .button("Project…")
                .on_hover_text("Choose the PS firmware and the PL design separately")
                .clicked()
            {
                self.project_open = !self.project_open;
            }
            ui.separator();
            if self.model.status.playing {
                if ui.button("Pause").clicked() {
                    self.model.pause();
                }
            } else if ui.button("Play").clicked() {
                self.model.play();
            }
            if ui
                .button("Step")
                .on_hover_text("Advance one quantum")
                .clicked()
            {
                self.model.step();
            }
            if ui.button("+100 ms").clicked() {
                self.model.run_for(100 * Ps::PER_MS);
            }
            egui::ComboBox::from_id_salt("speed")
                .selected_text(SPEEDS[self.model.speed].0)
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in SPEEDS.iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.model.speed, i, *label)
                            .clicked()
                            && self.model.status.playing
                        {
                            self.model.play();
                        }
                    }
                });
            ui.separator();
            ui.label(RichText::new(format!("t = {}", Ps(self.model.status.now_ps))).monospace());
            ui.separator();
            ui.label("seed");
            ui.add(egui::TextEdit::singleline(&mut self.model.seed_text).desired_width(64.0));
            if ui.button("Reset").clicked() {
                self.model.reset();
            }
            ui.separator();
            if ui.button("Save snapshot").clicked() {
                self.model.snapshot_save();
            }
            if ui.button("Restore").clicked() {
                self.model.snapshot_restore();
            }
        });
    }

    fn badge(&self, ui: &mut egui::Ui) {
        let s = &self.model.status;
        let (text, color) = match s.fidelity {
            sim_api::Fidelity::Demo => (
                "FIDELITY: DEMO (no CPU / RTL)",
                Color32::from_rgb(235, 160, 20),
            ),
            sim_api::Fidelity::F0 => (
                "FIDELITY: F0 functional + icount",
                Color32::from_rgb(90, 180, 255),
            ),
            sim_api::Fidelity::F1 => ("FIDELITY: F1 annotated", Color32::from_rgb(90, 220, 140)),
            sim_api::Fidelity::F2 => ("FIDELITY: F2 detailed", Color32::from_rgb(90, 220, 140)),
        };
        ui.label(RichText::new(text).color(color).strong());
        ui.label(format!(
            "PS backend: {}   PL backend: {}",
            s.ps_backend, s.pl_backend
        ));
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("System");
        self.badge(ui);
        let s = &self.model.status;
        ui.label(format!("board: {}", s.board));
        ui.label(format!("quantum: {}   seed: {}", Ps(s.quantum_ps), s.seed));
        ui.label(format!("pending events: {}", s.pending_events));
        ui.label(RichText::new(&s.trace_hash).monospace().small());
        for line in self.model.storage_lines() {
            ui.label(RichText::new(line).small());
        }
        ui.separator();

        for core in 0..2usize {
            let stopped = self.model.cpu_regs[core].is_some();
            egui::CollapsingHeader::new(format!(
                "CPU {core}{}",
                if stopped { " (stopped)" } else { "" }
            ))
            .id_salt(("cpu", core))
            .default_open(stopped)
            .show(ui, |ui| match &self.model.cpu_regs[core] {
                Some(r) => {
                    if let Some(s) = &r.pc_symbol {
                        ui.label(RichText::new(format!("pc in {s}")).strong());
                    }
                    for row in r.r.chunks(4).enumerate() {
                        let text: Vec<String> = row
                            .1
                            .iter()
                            .enumerate()
                            .map(|(i, v)| {
                                let n = row.0 * 4 + i;
                                let name = match n {
                                    13 => "sp".to_string(),
                                    14 => "lr".to_string(),
                                    15 => "pc".to_string(),
                                    n => format!("r{n}"),
                                };
                                format!("{name:>3} {v:08x}")
                            })
                            .collect();
                        ui.label(RichText::new(text.join("  ")).monospace().small());
                    }
                    ui.label(
                        RichText::new(format!("cpsr {:08x}  mode {}", r.cpsr, r.mode))
                            .monospace()
                            .small(),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(&self.cpu_reason[core])
                            .color(Color32::GRAY)
                            .small(),
                    );
                }
            });
        }
        egui::CollapsingHeader::new("PL (bus accesses)").show(ui, |ui| {
            let bus: Vec<&TraceRecord> = self
                .model
                .trace_tail
                .iter()
                .filter(|r| matches!(r.event, TraceEvent::Bus { .. }))
                .collect();
            if bus.is_empty() {
                ui.label(RichText::new(&self.pl_reason).color(Color32::GRAY).small());
            } else {
                for r in bus {
                    ui.label(RichText::new(describe(r)).monospace().small());
                }
                ui.label(
                    RichText::new("PL signals and AXI monitors: see docs/STATUS.md")
                        .color(Color32::GRAY)
                        .small(),
                );
            }
        });
        if !self.model.device_images.is_empty() {
            egui::CollapsingHeader::new("Pmod devices")
                .default_open(true)
                .show(ui, |ui| self.devices_panel(ui));
        }
        if self.model.has_media() {
            egui::CollapsingHeader::new("HDMI")
                .default_open(true)
                .show(ui, |ui| self.hdmi_panel(ui));
            egui::CollapsingHeader::new("Audio codec")
                .default_open(true)
                .show(ui, |ui| self.audio_panel(ui));
        }
        if let Some(n) = &self.model.net {
            egui::CollapsingHeader::new("Ethernet (GEM0)")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(format!("{} on {} via {}", n.mac, n.subnet, n.gateway));
                    ui.label(format!(
                        "tx {} frame(s) / {} B, rx {} frame(s) / {} B",
                        n.frames_tx, n.bytes_tx, n.frames_rx, n.bytes_rx
                    ));
                    ui.label(format!(
                        "arp {}  icmp {}  udp {}  tcp {}  other {}",
                        n.arp, n.icmp, n.udp, n.tcp, n.other
                    ));
                    if n.bad_checksums > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{} frame(s) with a bad checksum",
                                n.bad_checksums
                            ))
                            .color(Color32::from_rgb(235, 160, 20)),
                        );
                    }
                    if !n.restricted {
                        ui.label(
                            RichText::new("open network: runs are not reproducible")
                                .color(Color32::from_rgb(235, 160, 20))
                                .small(),
                        );
                    }
                });
        }
        egui::CollapsingHeader::new("Memory / image viewer")
            .default_open(self.mem_open)
            .show(ui, |ui| self.memory_panel(ui));
        egui::CollapsingHeader::new("Clock tree, IRQ map, SLCR").show(ui, |ui| {
            ui.label(
                RichText::new(
                    "Not available: these panels need a PS/PL backend (milestone P1). \
                         See docs/STATUS.md.",
                )
                .color(Color32::GRAY)
                .small(),
            );
        });
        ui.separator();
        ui.strong(format!("Trace ({} records)", self.model.trace_total));
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("trace")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for r in &self.model.trace_tail {
                    ui.label(RichText::new(describe(r)).monospace().small());
                }
            });
    }

    /// Guest memory as a hex dump or as a grayscale image.
    fn memory_panel(&mut self, ui: &mut egui::Ui) {
        if !self.model.can_read_memory() {
            ui.label(
                RichText::new("Needs a PS backend (qemu): the demo engine has no guest memory.")
                    .color(Color32::GRAY)
                    .small(),
            );
            return;
        }
        let mut read = self.mem_auto && self.frames % 15 == 0;
        ui.horizontal(|ui| {
            ui.label("addr");
            ui.add(egui::TextEdit::singleline(&mut self.mem_addr).desired_width(86.0));
            if ui.button("Read").clicked() {
                read = true;
            }
            ui.checkbox(&mut self.mem_auto, "live");
        });
        ui.horizontal(|ui| {
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
        if self.mem_view != MemView::Hex {
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
            MemView::Hex => {
                ui.label(RichText::new(&self.mem_text).monospace().small());
            }
            _ => {
                if let Some(tex) = &self.mem_tex {
                    let size = tex.size_vec2();
                    let scale = (260.0 / size.x.max(size.y)).max(1.0).floor();
                    ui.add(egui::Image::new((tex.id(), size * scale)));
                }
            }
        }
    }

    fn read_memory(&mut self, ctx: &egui::Context) {
        self.mem_error = None;
        let parse = |s: &str| -> Option<u64> {
            let t = s.trim();
            match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => t.parse().ok(),
            }
        };
        let Some(addr) = parse(&self.mem_addr) else {
            self.mem_error = Some("address is not a number".to_string());
            return;
        };
        match self.mem_view {
            MemView::Hex => match self.model.mem_read(addr, 256) {
                Ok(d) => {
                    self.mem_text = d
                        .chunks(16)
                        .enumerate()
                        .map(|(i, row)| {
                            let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
                            format!("{:08x}  {}", addr + (i * 16) as u64, hex.join(" "))
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                Err(e) => self.mem_error = Some(e),
            },
            view => {
                let (Some(w), Some(h)) = (parse(&self.mem_w), parse(&self.mem_h)) else {
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

    fn console(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.console_tab, 0, "UART0");
            ui.selectable_value(&mut self.console_tab, 1, "UART1");
            ui.checkbox(&mut self.crlf, "send CR LF");
            ui.label(
                RichText::new("target output shown; typed text goes to the target")
                    .small()
                    .color(Color32::GRAY),
            );
        });
        let port = self.console_tab.min(1);
        egui::ScrollArea::vertical()
            .id_salt(("console", port))
            .stick_to_bottom(true)
            .max_height(ui.available_height() - 34.0)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(self.model.console[port].replace('\r', "")).monospace(),
                    )
                    .wrap(),
                );
            });
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.console_input)
                    .desired_width(ui.available_width() - 70.0)
                    .hint_text("type and press Enter"),
            );
            let send = ui.button("Send").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            if send && !self.console_input.is_empty() {
                let text = std::mem::take(&mut self.console_input);
                self.model.send_uart(port as u8, &text, self.crlf);
                edit.request_focus();
            }
        });
    }

    fn board_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let def = &self.model.def;
        let (lw, lh) = (def.layout.width, def.layout.height);
        let avail = ui.available_size();
        let fit = (avail.x / lw).min(avail.y / lh).clamp(0.3, 3.0);
        let render_scale = (fit * ctx.pixels_per_point()).clamp(0.25, 4.0);
        let key = (self.model.state.clone(), (render_scale * 1000.0) as u32);
        if self.drawn.as_ref() != Some(&key) || self.texture.is_none() {
            let img = render(def, &self.model.state, render_scale);
            let color = ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.rgba,
            );
            match &mut self.texture {
                Some(t) => t.set(color, TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ctx.load_texture("board", color, TextureOptions::LINEAR));
                }
            }
            self.drawn = Some(key);
        }
        let Some(tex) = &self.texture else { return };

        let (rect, resp) =
            ui.allocate_exact_size(vec2(lw * fit, lh * fit), Sense::click_and_drag());
        self.board_rect = Some(rect);
        self.board_fit = fit;
        let painter = ui.painter_at(rect);
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // Text labels are drawn here: the shared software renderer has no font.
        let font = FontId::proportional((11.0 * fit).clamp(8.0, 16.0));
        let at = |x: f32, y: f32| rect.min + vec2(x * fit, y * fit);
        let label_col = Color32::from_rgb(225, 232, 228);
        for s in &def.switches {
            let text = match &s.key {
                Some(k) => format!("{} [{k}]", s.id.to_uppercase()),
                None => s.id.to_uppercase(),
            };
            painter.text(
                at(s.x, s.y + 44.0),
                Align2::CENTER_CENTER,
                text,
                font.clone(),
                label_col,
            );
        }
        for b in &def.buttons {
            painter.text(
                at(b.x, b.y + 36.0),
                Align2::CENTER_CENTER,
                b.id.to_uppercase(),
                font.clone(),
                label_col,
            );
            if let Some(k) = &b.key {
                painter.text(
                    at(b.x, b.y),
                    Align2::CENTER_CENTER,
                    k,
                    font.clone(),
                    Color32::BLACK,
                );
            }
        }
        for l in &def.leds {
            painter.text(
                at(l.x, l.y - 28.0),
                Align2::CENTER_CENTER,
                l.id.to_uppercase(),
                font.clone(),
                label_col,
            );
        }
        for l in &def.rgb_leds {
            painter.text(
                at(l.x, l.y - 30.0),
                Align2::CENTER_CENTER,
                l.id.to_uppercase(),
                font.clone(),
                label_col,
            );
        }
        for p in &def.pmods {
            painter.text(
                at(p.x, p.y - 32.0),
                Align2::CENTER_CENTER,
                format!("Pmod {}", p.id),
                font.clone(),
                label_col,
            );
        }
        for c in &def.connectors {
            // Dark parts get light text, light connectors get dark text.
            let dark = matches!(
                c.kind.as_str(),
                "soc" | "ddr" | "qspi" | "hdmi_rx" | "hdmi_tx" | "power"
            );
            let col = if dark {
                Color32::from_rgb(220, 226, 235)
            } else {
                Color32::from_rgb(20, 22, 26)
            };
            painter.text(
                at(c.x, c.y),
                Align2::CENTER_CENTER,
                &c.label,
                font.clone(),
                col,
            );
        }

        // Pointer interaction.
        let hover = resp
            .hover_pos()
            .map(|p| ((p.x - rect.min.x) / fit, (p.y - rect.min.y) / fit));
        let hit = hover.and_then(|(x, y)| hit_test(def, x, y));
        let (pressed, released) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        if pressed {
            match hit {
                Some(Hit::Switch(i)) => self.model.toggle_switch(i),
                Some(Hit::Button(i)) => {
                    self.held_button = Some(i);
                    self.model.set_button(i, true);
                }
                _ => {}
            }
        }
        if released {
            if let Some(i) = self.held_button.take() {
                self.model.set_button(i, false);
            }
        }
        if hit.is_some() {
            ui.ctx().set_cursor_icon(match hit {
                Some(Hit::Switch(_) | Hit::Button(_)) => egui::CursorIcon::PointingHand,
                _ => egui::CursorIcon::Default,
            });
        }
        if let Some(h) = hit {
            let text = self.tooltip(h);
            resp.on_hover_text_at_pointer(text);
        }
    }

    fn tooltip(&self, h: Hit) -> String {
        let def = &self.model.def;
        let st = &self.model.state;
        let pin = |p: &Option<String>| match p {
            Some(p) => format!("pin {p} (unverified)"),
            None => "pin unknown".to_string(),
        };
        match h {
            Hit::Switch(i) => format!(
                "{}: {}\n{}",
                def.switches[i].id,
                if st.switches[i] { "on" } else { "off" },
                pin(&def.switches[i].pin)
            ),
            Hit::Button(i) => format!(
                "{}: {}\n{}",
                def.buttons[i].id,
                if st.buttons[i] { "pressed" } else { "released" },
                pin(&def.buttons[i].pin)
            ),
            Hit::Led(i) => format!(
                "{}: duty {}/255\n{}",
                def.leds[i].id,
                st.leds[i],
                pin(&def.leds[i].pin)
            ),
            Hit::Rgb(i) => {
                let c = st.rgb[i];
                format!("{}: r={} g={} b={}", def.rgb_leds[i].id, c[0], c[1], c[2])
            }
            Hit::Pmod(i) => {
                let levels: String = st.pmod_pins[i].iter().map(|l| l.symbol()).collect();
                format!(
                    "Pmod {} ({:?}): pins {levels}\n(pin locations unverified)",
                    def.pmods[i].id, def.pmods[i].domain
                )
            }
            Hit::Connector(i) => format!(
                "{} ({})\nDrawn only: no device model behind it yet (milestone P2).",
                def.connectors[i].label, def.connectors[i].kind
            ),
        }
    }

    /// Called once after the simulation was replaced by a freshly built one.
    fn engine_changed(&mut self) {
        self.seen_generation = self.model.generation;
        self.cpu_reason = [
            self.model.cpu_unavailable_reason(0),
            self.model.cpu_unavailable_reason(1),
        ];
        self.pl_reason = self.model.pl_unavailable_reason();
        self.key_down = vec![false; self.model.def.buttons.len()];
        self.held_button = None;
        self.drawn = None;
        self.drawn3d = None;
        self.scene = None;
        self.media.tex = None;
        self.media.tex_frame = None;
        self.mem_tex = None;
        if self.autoplay {
            self.autoplay = false;
            self.model.play();
        }
    }

    /// A banner while an engine is being built (XSim compiles the design).
    fn loading_overlay(&mut self, ctx: &egui::Context) {
        let Some((what, elapsed)) = self.model.loading() else {
            return;
        };
        let text = format!(
            "Building {what}… {:.0} s (the RTL is compiled by Vivado, which takes a while)",
            elapsed.as_secs_f32()
        );
        egui::Area::new(egui::Id::new("loading"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 70.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(text);
                    });
                });
            });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn lifecycle(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        if let Some(path) = self.opts.screenshot.clone() {
            // Give the first frames time to render before capturing.
            if self.frames >= 30 && !self.screenshot_requested {
                self.screenshot_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
                let png =
                    sim_api::board::encode_rgba_png(img.width() as u32, img.height() as u32, rgba);
                match png.and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
                    Ok(()) => eprintln!("zynqsim-gui: wrote {}", path.display()),
                    Err(e) => eprintln!("zynqsim-gui: screenshot failed: {e}"),
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if let Some(d) = self.opts.exit_after {
            if self.started.elapsed() >= d {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl App {
    /// One frame of the whole UI. Separate from `eframe::App::update` so tests
    /// can drive it with synthetic input and no window.
    fn ui(&mut self, ctx: &egui::Context) {
        self.model.refresh();
        if self.model.generation != self.seen_generation {
            self.engine_changed();
        }
        self.pump_audio();
        self.handle_keys(ctx);

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(4.0);
            self.control_bar(ui);
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status")
            .exact_height(22.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    match self
                        .model
                        .load_error
                        .as_ref()
                        .or(self.model.last_error.as_ref())
                    {
                        Some(e) => {
                            ui.colored_label(
                                Color32::from_rgb(240, 100, 90),
                                e.lines().next().unwrap_or(""),
                            );
                        }
                        None => {
                            ui.label(
                                RichText::new(
                                    "ready  |  keys: 1-4 switches, Q/W/E/R buttons (hold)",
                                )
                                .color(Color32::GRAY),
                            );
                        }
                    }
                });
            });
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(170.0)
            .show(ctx, |ui| self.console(ui));
        egui::SidePanel::right("info")
            .default_width(300.0)
            .show(ctx, |ui| self.side_panel(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            self.view_tabs(ui);
            ui.add_space(2.0);
            if self.view_3d {
                self.board_view_3d(ui, ctx);
            } else {
                self.board_view(ui, ctx);
            }
        });
        self.project_window(ctx);
        self.loading_overlay(ctx);

        self.lifecycle(ctx);
        // The simulation never depends on frame rate; repaint at a modest rate
        // so LEDs and the console track a running simulation.
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// Maps a board-file key label (`"1"`, `"Q"`) to an egui key.
fn key_from_label(label: &str) -> Option<Key> {
    match label {
        "0" => Some(Key::Num0),
        "1" => Some(Key::Num1),
        "2" => Some(Key::Num2),
        "3" => Some(Key::Num3),
        "4" => Some(Key::Num4),
        "5" => Some(Key::Num5),
        "6" => Some(Key::Num6),
        "7" => Some(Key::Num7),
        "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        other => Key::from_name(&other.to_uppercase()),
    }
}

/// One line of the trace panel.
fn describe(r: &TraceRecord) -> String {
    let body = match &r.event {
        TraceEvent::Input { what } => format!("input  {what}"),
        TraceEvent::Signal { name, value, width } => {
            format!("signal {name} = {value:#x} ({width}b)")
        }
        TraceEvent::Uart { port, tx, byte } => {
            let c = if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte).to_string()
            } else {
                format!("\\x{byte:02x}")
            };
            format!("uart{port} {} {c}", if *tx { "tx" } else { "rx" })
        }
        TraceEvent::Marker { text } => format!("mark   {text}"),
        TraceEvent::Bus {
            port,
            write,
            addr,
            len,
            data,
            ok,
        } => format!(
            "bus    {port} {} {addr:#010x} len {len} data {data:#x}{}",
            if *write { "wr" } else { "rd" },
            if *ok { "" } else { " ERROR" }
        ),
    };
    format!("{:>14}  {body}", Ps(r.t.0).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_key_labels_map_to_egui_keys() {
        assert_eq!(key_from_label("1"), Some(Key::Num1));
        assert_eq!(key_from_label("4"), Some(Key::Num4));
        assert_eq!(key_from_label("Q"), Some(Key::Q));
        assert_eq!(key_from_label("r"), Some(Key::R));
        assert_eq!(key_from_label("???"), None);
    }

    #[test]
    fn every_key_in_the_builtin_board_is_bound() {
        let def = sim_api::board::BoardDef::zybo_z7_20();
        for k in def
            .switches
            .iter()
            .filter_map(|s| s.key.as_deref())
            .chain(def.buttons.iter().filter_map(|b| b.key.as_deref()))
        {
            assert!(key_from_label(k).is_some(), "key {k} has no egui binding");
        }
    }

    // ---- driving the whole UI with synthetic input, no window ----

    use eframe::egui::{Event, Modifiers, MouseWheelUnit, PointerButton};
    use sim_api::SimClient;
    use sim_api::SimConfig;

    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
    }

    impl Harness {
        fn new() -> Harness {
            let model = Model::new(SimClient::spawn(SimConfig::default()).unwrap()).unwrap();
            let mut h = Harness {
                ctx: egui::Context::default(),
                app: App::new(model, Opts::default()),
                time: 0.0,
            };
            // Two frames: the first lays everything out.
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0))),
                events,
                time: Some(self.time),
                ..Default::default()
            };
            let app = &mut self.app;
            let _ = self.ctx.run(input, |ctx| app.ui(ctx));
        }

        fn click(&mut self, p: egui::Pos2) {
            self.frame(vec![Event::PointerMoved(p)]);
            self.press(p);
            self.release(p);
        }

        fn press(&mut self, p: egui::Pos2) {
            self.frame(vec![Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }]);
        }

        fn release(&mut self, p: egui::Pos2) {
            self.frame(vec![Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]);
        }

        fn key(&mut self, key: Key, pressed: bool) {
            self.frame(vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            }]);
        }

        /// Screen position of a point given in 2D layout units.
        fn at_layout(&self, x: f32, y: f32) -> egui::Pos2 {
            let r = self.app.board_rect.expect("the board was drawn");
            r.min + vec2(x * self.app.board_fit, y * self.app.board_fit)
        }
    }

    #[test]
    fn clicking_a_switch_in_the_2d_view_toggles_it() {
        let mut h = Harness::new();
        let s = h.app.model.def.switches[1].clone();
        assert!(!h.app.model.state.switches[1]);
        h.click(h.at_layout(s.x, s.y));
        h.frame(vec![]);
        assert!(
            h.app.model.state.switches[1],
            "switch 1 is on after a click"
        );
        assert_eq!(h.app.model.state.leds[1], 255, "and the demo drove LED1");
        h.click(h.at_layout(s.x, s.y));
        h.frame(vec![]);
        assert!(
            !h.app.model.state.switches[1],
            "a second click turns it off"
        );
        // A click on empty PCB does nothing.
        h.click(h.at_layout(500.0, 40.0));
        assert!(h.app.model.state.switches.iter().all(|&s| !s));
    }

    #[test]
    fn a_button_is_held_while_the_pointer_is_down() {
        let mut h = Harness::new();
        let b = h.app.model.def.buttons[2].clone();
        let p = h.at_layout(b.x, b.y);
        h.frame(vec![Event::PointerMoved(p)]);
        h.press(p);
        h.frame(vec![]);
        assert!(h.app.model.state.buttons[2], "pressed while held");
        h.release(p);
        h.frame(vec![]);
        assert!(!h.app.model.state.buttons[2], "released with the pointer");
    }

    #[test]
    fn keyboard_shortcuts_toggle_switches_and_hold_buttons() {
        let mut h = Harness::new();
        h.key(Key::Num1, true);
        h.key(Key::Num1, false);
        h.frame(vec![]);
        assert!(h.app.model.state.switches[0], "key 1 toggles switch 0");
        h.key(Key::Q, true);
        h.frame(vec![]);
        assert!(h.app.model.state.buttons[0], "Q holds button 0");
        h.key(Key::Q, false);
        h.frame(vec![]);
        assert!(!h.app.model.state.buttons[0]);
    }

    #[test]
    fn the_3d_view_orbits_zooms_and_picks() {
        let mut h = Harness::new();
        h.app.view_3d = true;
        h.frame(vec![]);
        h.frame(vec![]);
        let rect = h.app.board_rect.expect("3D view drawn");
        let scene_pt = |h: &Harness, hit| {
            let sc = h.app.scene.as_ref().unwrap();
            let (x, y) = sc.centre_of(hit).expect("control visible");
            let r = h.app.board_rect.unwrap();
            pos2(
                r.min.x + x / sc.image.width as f32 * r.width(),
                r.min.y + y / sc.image.height as f32 * r.height(),
            )
        };

        // A click on a drawn switch toggles that switch.
        let p = scene_pt(&h, Hit::Switch(2));
        h.click(p);
        h.frame(vec![]);
        assert!(h.app.model.state.switches[2], "3D click reaches the switch");

        // Dragging empty background rotates the camera.
        let before = h.app.cam;
        let from = rect.min + vec2(12.0, 12.0);
        h.frame(vec![Event::PointerMoved(from)]);
        h.press(from);
        h.frame(vec![Event::PointerMoved(from + vec2(40.0, 25.0))]);
        h.frame(vec![Event::PointerMoved(from + vec2(90.0, 50.0))]);
        h.release(from + vec2(90.0, 50.0));
        assert_ne!(h.app.cam.yaw, before.yaw, "yaw changed by the drag");
        assert_ne!(h.app.cam.pitch, before.pitch, "pitch changed by the drag");
        assert!(
            h.app.model.state.switches[2],
            "the drag did not click anything"
        );

        // Scrolling zooms.
        let z0 = h.app.cam.zoom;
        let centre = rect.center();
        h.frame(vec![Event::PointerMoved(centre)]);
        for _ in 0..4 {
            h.frame(vec![Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, 80.0),
                modifiers: Modifiers::NONE,
            }]);
        }
        for _ in 0..6 {
            h.frame(vec![]);
        }
        assert!(h.app.cam.zoom > z0, "zoom {} -> {}", z0, h.app.cam.zoom);
    }

    #[test]
    fn the_project_window_builds_and_swaps_the_simulation() {
        let mut h = Harness::new();
        h.app.project_open = true;
        h.frame(vec![]);
        assert_eq!(h.app.model.generation, 0);

        // A valid draft builds in the background and replaces the engine.
        h.app.draft.quantum_ns = "2000".to_string();
        h.app.build_draft("test");
        assert!(h.app.model.loading().is_some(), "a build was started");
        let start = Instant::now();
        while h.app.model.generation == 0 {
            h.frame(vec![]);
            assert!(start.elapsed() < Duration::from_secs(20), "build finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.app.model.loading().is_none());
        assert_eq!(
            h.app.model.status.quantum_ps, 2_000_000,
            "new settings apply"
        );
        assert!(h.app.model.load_error.is_none());

        // An invalid draft is refused with a message and starts nothing.
        h.app.draft.use_qemu = true;
        h.app.draft.cpu0.clear();
        h.app.build_draft("bad");
        assert!(h.app.draft_msg.is_some());
        assert!(h.app.model.loading().is_none());

        // Opening a project file by path works the same way.
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/demo/project.toml");
        let g = h.app.model.generation;
        h.app.open_project_file(file);
        let start = Instant::now();
        while h.app.model.generation == g {
            h.frame(vec![]);
            assert!(start.elapsed() < Duration::from_secs(20), "open finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.app.model.status.ps_backend, "demo");
        // Opening a file that is not a project reports why.
        h.app
            .open_project_file(std::path::PathBuf::from("no/such/project.toml"));
        assert!(h.app.draft_msg.is_some());
    }

    #[test]
    fn trace_lines_are_readable() {
        let r = TraceRecord {
            t: Ps(1_500_000),
            event: TraceEvent::Uart {
                port: 1,
                tx: true,
                byte: b'A',
            },
        };
        assert!(describe(&r).ends_with("uart1 tx A"), "{}", describe(&r));
        let r = TraceRecord {
            t: Ps(0),
            event: TraceEvent::Uart {
                port: 0,
                tx: false,
                byte: 0x0d,
            },
        };
        assert!(describe(&r).contains("\\x0d"));
    }
}
