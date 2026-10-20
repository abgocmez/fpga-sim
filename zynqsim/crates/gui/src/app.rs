//! The egui front end: board view, control bar, console and info panels.

mod cpu;
mod debugger;
mod dialogs;
mod media;
mod memory;
mod project;
mod sections;
mod shell;
mod view3d;
mod wave_view;

use crate::loader::ProjectDraft;
use crate::model::{Model, SPEEDS, TailFollow, TraceFilter, TraceKind, trace_kind, visible_lines};
use crate::prefs::Prefs;
use crate::waves::elide_long_symbols;
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Rect, RichText, Sense, TextureHandle,
    TextureOptions, pos2, vec2,
};
use sections::{SectionState, section};
use shell::Action;
use sim_api::Ps;
use sim_api::board::{BoardState, Camera, Hit, hit_test, render};
use sim_api::trace::{TraceEvent, TraceRecord};
use std::time::{Duration, Instant};

/// The key the side panel's section state is saved under in eframe's storage.
pub const SECTIONS_KEY: &str = "zynqsim.sections";

/// Start-up options that exist for automated checks.
#[derive(Default)]
pub struct Opts {
    /// Close the window after this many milliseconds.
    pub exit_after: Option<Duration>,
    /// Save a PNG of the window here, then exit.
    pub screenshot: Option<std::path::PathBuf>,
    /// Open the memory viewer at this address as a 64x64 image.
    pub mem: Option<u64>,
    /// Show the memory viewer's hex view instead of the image.
    pub mem_hex: bool,
    /// Open on the 3D board view.
    pub view_3d: bool,
    /// Open on the Waveforms tab, with these signals shown.
    pub waves: Vec<String>,
    /// Open on the Waveforms tab with the default signals.
    pub waves_tab: bool,
    /// Text the trace strip's filter starts with.
    pub trace_text: String,
    /// Open the project window.
    pub project_window: bool,
    /// Scroll the side panel's sections down by this many points, so a screenshot can show a
    /// section that is lower down (0 keeps the top).
    pub side_scroll: f32,
    /// Where relative paths of the project that was opened resolve, and the
    /// project file itself, for the project window.
    pub project: Option<std::path::PathBuf>,
    /// Settings to start with (window size, UI scale, theme, recent projects).
    pub prefs: Prefs,
    /// Where the settings are saved as they change; `None` keeps them in memory.
    pub prefs_path: Option<std::path::PathBuf>,
    /// Open the keyboard shortcuts window.
    pub show_shortcuts: bool,
    /// Restart the project under the debugger at start-up, and save the
    /// screenshot only once it has stopped (`--debugger`).
    pub restart_debugger: bool,
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
    mem_unit: memory::Unit,
    mem_data: Option<(u64, Vec<u8>)>,
    mem_changed: Vec<bool>,
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
    xsa_requested: bool,
    autoplay: bool,
    seen_generation: u32,
    // media panels
    media: media::MediaUi,
    // waveform tab
    wave_ui: wave_view::WaveUi,
    // trace strip and console
    trace_filter: TraceFilter,
    trace_follow: TailFollow,
    /// Text size of the trace strip and the console, in points (10..=18).
    font_pt: f32,
    /// Share of the side panel's height given to the trace strip (dragged with the grip).
    trace_share: f32,
    // menu bar, status bar, settings
    shell: shell::Shell,
    /// Which side-panel sections are open (remembered between runs).
    sections: SectionState,
    /// Set while the frame draws the debugger offer, and where its restart
    /// button was drawn (the tests look at both).
    #[cfg_attr(not(test), allow(dead_code))]
    offer_drawn: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    offer_button: Option<Rect>,
    /// The `--debugger` restart has been asked for.
    restart_requested: bool,
}

impl App {
    /// Builds the app around a loaded model.
    pub fn new(model: Model, opts: Opts) -> App {
        let opts_mem = opts.mem;
        let mut shell = shell::Shell::new(
            opts.prefs.clone(),
            opts.prefs_path.clone(),
            opts.show_shortcuts,
        );
        if let Some(f) = &opts.project {
            shell.note_recent(f);
        }
        let opts_mem_hex = opts.mem_hex;
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
        let waves = !opts.waves.is_empty() || opts.waves_tab;
        let mut model = model;
        model.waves.selected.extend(opts.waves.iter().cloned());
        let opts_project_window = opts.project_window;
        let opts_trace_text = opts.trace_text.clone();
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
            mem_view: if opts_mem.is_some() && !opts_mem_hex {
                MemView::ImageWords
            } else {
                MemView::Hex
            },
            mem_auto: opts_mem.is_some(),
            mem_unit: memory::Unit::Bits8,
            mem_data: None,
            mem_changed: Vec::new(),
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
            xsa_requested: false,
            autoplay: false,
            seen_generation: 0,
            media: media::MediaUi::default(),
            wave_ui: wave_view::WaveUi {
                tab: waves,
                ..Default::default()
            },
            trace_filter: TraceFilter {
                text: opts_trace_text,
                ..TraceFilter::default()
            },
            trace_follow: TailFollow::default(),
            font_pt: 12.0,
            trace_share: 0.35,
            shell,
            sections: SectionState::default(),
            offer_drawn: false,
            offer_button: None,
            restart_requested: false,
        }
    }

    /// The status-bar hint listing the board's keyboard shortcuts.
    fn key_help(&self) -> String {
        let keys = |it: Vec<Option<&str>>| {
            let k: Vec<&str> = it.into_iter().flatten().collect();
            k.join("/")
        };
        let def = &self.model.def;
        format!(
            "ready  |  keys: {} switches, {} buttons (hold)",
            keys(def.switches.iter().map(|s| s.key.as_deref()).collect()),
            keys(def.buttons.iter().map(|b| b.key.as_deref()).collect()),
        )
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
        // Ctrl/Cmd combinations belong to the shell (Ctrl+O, Ctrl+= ...), so
        // a board key pressed with one of them does nothing.
        for (i, k) in switch_keys.iter().enumerate() {
            if let Some(k) = k {
                if ctx.input(|inp| inp.key_pressed(*k) && !inp.modifiers.command) {
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
                let down = ctx.input(|inp| inp.key_down(*k) && !inp.modifiers.command);
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
                .button("New from XSA…")
                .on_hover_text("Import a Vivado hardware handoff (XSA or HWH) into a new project")
                .clicked()
            {
                self.xsa_requested = true;
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
            } else if ui
                .button(self.run_label())
                .on_hover_text(if self.halted() {
                    "Resume the guest; it runs on to the next breakpoint"
                } else {
                    "Run the simulation in real time"
                })
                .clicked()
            {
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
        // The sections scroll, so a long one (Source, HDMI) cannot push the
        // trace off the bottom of the window; the trace keeps a strip of its own.
        let avail = ui.available_height();
        const GRIP: f32 = 8.0;
        let trace_h = (avail * self.trace_share).clamp(120.0, (avail - 120.0).max(120.0));
        let mut sections = egui::ScrollArea::vertical()
            .id_salt("side_sections")
            .max_height((avail - trace_h - GRIP).max(0.0))
            .auto_shrink([false, false]);
        if self.opts.side_scroll > 0.0 {
            sections = sections.vertical_scroll_offset(self.opts.side_scroll);
        }
        sections.show(ui, |ui| self.side_sections(ui));
        // Drag grip between the sections and the trace strip.
        let (rect, grip) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), GRIP), egui::Sense::drag());
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        if grip.hovered() || grip.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if grip.dragged() && avail > 0.0 {
            self.trace_share = (self.trace_share - grip.drag_delta().y / avail).clamp(0.3, 0.7);
        }
        self.trace_panel(ui);
    }

    /// The play-speed selector and the measured sim/wall ratio.
    fn speed_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("speed");
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
            let text = if !self.model.status.playing {
                "paused".to_string()
            } else {
                match self.model.speed_meter.ratio() {
                    Some(r) => format!("measured {r:.2}x sim/wall"),
                    None => "measuring...".to_string(),
                }
            };
            ui.label(RichText::new(text).monospace().small());
        });
    }

    /// Everything above the trace.
    fn side_sections(&mut self, ui: &mut egui::Ui) {
        // Taken out while the section bodies, which borrow self, run.
        let mut sections = std::mem::take(&mut self.sections);
        self.speed_row(ui);
        ui.separator();
        ui.heading("System");
        self.badge(ui);
        let s = &self.model.status;
        ui.label(format!("board: {}", s.board));
        ui.label(format!("quantum: {}   seed: {}", Ps(s.quantum_ps), s.seed));
        ui.label(format!("pending events: {}", s.pending_events));
        ui.label(RichText::new(&s.trace_hash).monospace().small());
        ui.separator();

        // Opened by itself while the registers are missing for want of the
        // debugger, so the offer to restart is in view.
        self.offer_drawn = false;
        self.offer_button = None;
        let stopped = self.model.cpu_regs[0].is_some();
        let cpu_summary = cpu::summary(
            self.model.cpu_regs[0].as_ref(),
            self.model.status.debug.is_some(),
        );
        let mut picked: Option<Action> = None;
        section(
            ui,
            &mut sections,
            "cpu",
            "CPU registers",
            &cpu_summary,
            stopped || self.restart_offered(),
            |ui| picked = self.cpu_registers(ui),
        );

        // The source line of the stopped pc, when the ELF has line info.
        let source = self.model.source.view.as_ref().map(|v| {
            format!(
                "{}:{}",
                v.file.rsplit('/').next().unwrap_or(v.file.as_str()),
                v.line
            )
        });
        let has_source = source.is_some();
        let source_summary = source.unwrap_or_else(|| "no line information".to_string());
        // The pc of the stopped core 0, for the panel's "no line" text.
        let stopped_pc = self.model.cpu_regs[0].as_ref().map(|r| {
            r.pc_symbol
                .clone()
                .unwrap_or_else(|| format!("pc {:#010x}", r.r[15]))
        });
        section(
            ui,
            &mut sections,
            "source",
            if has_source {
                "Source (stopped)"
            } else {
                "Source"
            },
            &source_summary,
            has_source || self.restart_offered(),
            |ui| {
                if !has_source && self.restart_offered() && picked.is_none() {
                    picked = self.debugger_button(ui);
                }
                crate::source_view::show(ui, &mut self.model.source, stopped_pc.as_deref());
            },
        );
        if let Some(a) = picked {
            let ctx = ui.ctx().clone();
            self.run_action(&ctx, a);
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

        // Every attached device, with or without a picture (a sensor has none).
        if !self.model.status.devices.is_empty() {
            let summary = self
                .model
                .status
                .devices
                .iter()
                .map(|d| format!("{} on {}", d.kind, media::pmod_port_name(d.port)))
                .collect::<Vec<_>>()
                .join(", ");
            section(
                ui,
                &mut sections,
                "pmod",
                "Pmod devices",
                &summary,
                true,
                |ui| self.devices_panel(ui),
            );
        }
        if self.model.has_media() {
            let (video, audio) = self
                .model
                .status
                .media
                .as_ref()
                .map_or((0, 0), |m| (m.video_frames, m.audio_frames));
            section(
                ui,
                &mut sections,
                "hdmi",
                "HDMI",
                &format!("TX {video} frame(s)"),
                true,
                |ui| self.hdmi_panel(ui),
            );
            section(
                ui,
                &mut sections,
                "audio",
                "Audio codec",
                &format!("DAC {audio} frame(s)"),
                true,
                |ui| self.audio_panel(ui),
            );
        }
        if let Some(n) = &self.model.net {
            let summary = format!(
                "{} frame(s), {}",
                n.frames_tx + n.frames_rx,
                if n.restricted {
                    "isolated LAN"
                } else {
                    "open network"
                }
            );
            section(
                ui,
                &mut sections,
                "ethernet",
                "Ethernet (GEM0)",
                &summary,
                true,
                |ui| {
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
                },
            );
        }
        if let Some(st) = self.model.status.storage.clone() {
            if st.sd.is_some() || st.qspi.is_some() {
                section(
                    ui,
                    &mut sections,
                    "storage",
                    "Storage",
                    &crate::model::storage_summary(&st),
                    true,
                    |ui| media::storage_table(ui, &st),
                );
            }
        }
        let mem_summary = self.memory_summary();
        let mem_open = self.mem_open;
        section(
            ui,
            &mut sections,
            "memory",
            "Memory / image viewer",
            &mem_summary,
            mem_open,
            |ui| self.memory_panel(ui),
        );
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
        self.sections = sections;
    }

    /// The newest trace records, in the bottom strip of the side panel: a
    /// text size, a substring filter and type toggles, copy of the visible
    /// lines, and auto-scroll that pauses when the user scrolls up.
    fn trace_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(format!("Trace ({} records)", self.model.trace_total));
            ui.add(
                egui::Slider::new(&mut self.font_pt, 10.0..=18.0)
                    .step_by(1.0)
                    .suffix(" pt")
                    .show_value(true),
            )
            .on_hover_text("text size of the trace strip and the console");
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.trace_filter.text)
                    .hint_text("filter (text, no regex)")
                    .desired_width(ui.available_width()),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.trace_filter.uart, "uart");
            ui.checkbox(&mut self.trace_filter.board, "board");
            ui.checkbox(&mut self.trace_filter.bus, "bus");
            ui.checkbox(&mut self.trace_filter.marker, "marker");
            ui.checkbox(&mut self.trace_filter.protocol, "protocol");
        });
        let lines: Vec<(TraceKind, String)> = self
            .model
            .trace_tail
            .iter()
            .map(|r| (trace_kind(r), describe(r)))
            .collect();
        let shown = visible_lines(&lines, &self.trace_filter);
        ui.horizontal(|ui| {
            if ui
                .small_button("copy visible")
                .on_hover_text("copy the lines shown below to the clipboard")
                .clicked()
            {
                let text: Vec<&str> = shown.iter().map(|&i| lines[i].1.as_str()).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
            if self.trace_follow.paused() {
                let n = self.trace_follow.new_since_pause(self.model.trace_total);
                let label = if n > 0 {
                    format!("jump to latest ({n} new)")
                } else {
                    "jump to latest".to_string()
                };
                if ui.small_button(label).clicked() {
                    self.trace_follow.jump_to_latest();
                }
            } else {
                ui.label(
                    RichText::new(format!("{} shown", shown.len()))
                        .small()
                        .color(Color32::GRAY),
                );
            }
        });
        ui.add_space(4.0);
        let font = FontId::monospace(self.font_pt);
        let out = egui::ScrollArea::vertical()
            .id_salt("trace")
            .auto_shrink([false, false])
            .stick_to_bottom(!self.trace_follow.paused())
            .show(ui, |ui| {
                for &i in &shown {
                    let line = &lines[i].1;
                    // A long mangled symbol is elided; the full line is the tooltip.
                    match elide_long_symbols(line) {
                        Some(short) => {
                            ui.label(RichText::new(short).font(font.clone()))
                                .on_hover_text(line.as_str());
                        }
                        None => {
                            ui.label(RichText::new(line.as_str()).font(font.clone()));
                        }
                    }
                }
            });
        let at_bottom = out.state.offset.y + out.inner_rect.height() >= out.content_size.y - 2.0;
        self.trace_follow.observe(at_bottom, self.model.trace_total);
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
                        RichText::new(self.model.console[port].replace('\r', ""))
                            .font(FontId::monospace(self.font_pt)),
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

    /// Restores the section state saved by an earlier run.
    pub fn restore_sections(&mut self, text: &str) {
        self.sections = SectionState::decode(text);
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
        self.mem_data = None;
        self.mem_changed.clear();
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
        if self.opts.restart_debugger && !self.restart_requested && self.frames >= 2 {
            self.restart_requested = true;
            self.restart_under_debugger(true);
        }
        if let Some(path) = self.opts.screenshot.clone() {
            // Give the first frames time to render before capturing; with
            // `--debugger`, wait for the restarted guest to stop at its breakpoint.
            let debugger_ready = !self.opts.restart_debugger
                || (self.restart_requested && self.model.load.is_none() && self.halted());
            if self.frames >= 30 && debugger_ready && !self.screenshot_requested {
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

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(SECTIONS_KEY, self.sections.encode());
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
        self.shell_frame(ctx);
        self.shell_keys(ctx);
        self.handle_keys(ctx);

        let picked = egui::TopBottomPanel::top("menu")
            .show(ctx, |ui| self.menu_bar(ui))
            .inner;
        for a in picked {
            self.run_action(ctx, a);
        }
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(4.0);
            self.control_bar(ui);
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status")
            .exact_height(24.0)
            .show(ctx, |ui| self.status_bar(ui));
        if self.shell.show_console {
            egui::TopBottomPanel::bottom("console")
                .resizable(true)
                .default_height(170.0)
                .show(ctx, |ui| self.console(ui));
        }
        if self.shell.show_side {
            egui::SidePanel::right("info")
                .default_width(300.0)
                .show(ctx, |ui| self.side_panel(ui));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            self.view_tabs(ui);
            ui.add_space(2.0);
            if self.wave_ui.tab {
                self.waves_view(ui);
            } else {
                self.model.waves.active = false;
                if self.view_3d {
                    self.board_view_3d(ui, ctx);
                } else {
                    self.board_view(ui, ctx);
                }
            }
        });
        self.project_window(ctx);
        self.shortcuts_window(ctx);
        self.about_window(ctx);
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
pub(super) mod tests {
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

    pub(super) struct Harness {
        pub(super) ctx: egui::Context,
        pub(super) app: App,
        time: f64,
    }

    impl Harness {
        pub(super) fn new() -> Harness {
            Harness::with_model(
                Model::new(SimClient::spawn(SimConfig::default()).unwrap()).unwrap(),
            )
        }

        pub(super) fn with_model(model: Model) -> Harness {
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

        pub(super) fn frame(&mut self, events: Vec<Event>) {
            self.frame_with(events, egui::Modifiers::NONE);
        }

        /// A frame with the given keyboard modifiers held (Ctrl is
        /// `Modifiers::COMMAND` on Windows).
        pub(super) fn frame_with(&mut self, events: Vec<Event>, modifiers: egui::Modifiers) {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0))),
                events,
                time: Some(self.time),
                modifiers,
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
    fn the_gui_new_from_xsa_imports_and_opens_the_project() {
        let mut h = Harness::new();
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let xsa = fixtures.join("xsa/zs_math_perf_mon.hwh");
        let temp_dir =
            std::env::temp_dir().join(format!("zynqsim-gui-xsa-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let test_hwh = temp_dir.join("test_math_gpio.hwh");
        std::fs::copy(&xsa, &test_hwh).unwrap();

        let g = h.app.model.generation;
        let opts = sim_api::xsa::ImportOptions {
            rtl: vec![fixtures.join("rtl/math/my_math_ip.v")],
            ps: Some("demo".to_string()),
            pl: Some("demo".to_string()),
            ..Default::default()
        };
        h.app.new_from_xsa_file_with_opts(test_hwh, opts);
        assert!(h.app.draft_msg.is_none(), "error: {:?}", h.app.draft_msg);
        wait_for_engine(&mut h, g);
        assert_eq!(h.app.draft.board_name, "zybo-z7-20");
        assert_eq!(h.app.model.status.ps_backend, "demo");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Waits until the model has adopted a freshly built engine.
    fn wait_for_engine(h: &mut Harness, from_generation: u32) {
        let start = Instant::now();
        while h.app.model.generation == from_generation {
            h.frame(vec![]);
            assert!(start.elapsed() < Duration::from_secs(20), "build finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            h.app.model.load_error.is_none(),
            "{:?}",
            h.app.model.load_error
        );
        h.frame(vec![]);
    }

    #[test]
    fn picking_a_board_in_the_project_window_changes_the_controls_and_they_work() {
        let mut h = Harness::new();
        h.app.project_open = true;
        h.frame(vec![]);
        assert_eq!(h.app.model.def.name, "zybo-z7-20");
        assert_eq!(h.app.model.state.buttons.len(), 4);
        assert!(
            h.app
                .key_help()
                .contains("1/2/3/4 switches, Q/W/E/R buttons")
        );

        // The picker writes `board_name` into the draft; Build swaps the engine.
        h.app.draft.board_name = "zedboard".to_string();
        let g = h.app.model.generation;
        h.app.build_draft("zedboard");
        wait_for_engine(&mut h, g);
        assert_eq!(h.app.model.def.name, "zedboard");
        assert_eq!(h.app.model.state.switches.len(), 8);
        assert_eq!(h.app.model.state.buttons.len(), 7);
        assert_eq!(h.app.model.state.leds.len(), 8);
        assert_eq!(h.app.key_down.len(), 7, "one held-key flag per button");
        assert!(h.app.key_help().contains("1/2/3/4/5/6/7/8 switches"));

        // A button the Zybo does not have, pressed with the pointer.
        let b = h.app.model.def.buttons[6].clone();
        let p = h.at_layout(b.x, b.y);
        h.frame(vec![Event::PointerMoved(p)]);
        h.press(p);
        h.frame(vec![]);
        assert!(h.app.model.state.buttons[6], "button 6 is down");
        h.release(p);
        h.frame(vec![]);
        assert!(!h.app.model.state.buttons[6], "and up again");

        // A switch the Zybo does not have, by clicking and by its key.
        let s = h.app.model.def.switches[7].clone();
        h.click(h.at_layout(s.x, s.y));
        h.frame(vec![]);
        assert!(
            h.app.model.state.switches[7],
            "switch 7 is on after a click"
        );
        assert_eq!(h.app.model.state.leds[6], 0, "LED6 follows SW6, not SW7");
        h.key(Key::Num7, true);
        h.key(Key::Num7, false);
        h.frame(vec![]);
        assert!(h.app.model.state.switches[6], "key 7 toggles switch 6");
        assert_eq!(
            h.app.model.state.leds[6], 255,
            "which the demo shows on LED6"
        );
        h.key(Key::U, true);
        h.frame(vec![]);
        assert!(h.app.model.state.buttons[6], "U holds the seventh button");
        h.key(Key::U, false);
        h.frame(vec![]);

        // A custom board file, the way the picker's "custom file" entry sets it.
        h.app.draft.board_file = "../../examples/custom-board/my-board.toml".to_string();
        h.app.draft.dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let g = h.app.model.generation;
        h.app.build_draft("custom");
        wait_for_engine(&mut h, g);
        assert_eq!(h.app.model.def.name, "lab-board");
        assert_eq!(h.app.model.state.leds.len(), 6);
        assert!(
            h.app.model.def.svg_source.is_some(),
            "the artwork came along"
        );

        // A bad board file is refused before anything starts.
        h.app.draft.board_file = "no-such-board.toml".to_string();
        let g = h.app.model.generation;
        h.app.build_draft("bad");
        assert_eq!(h.app.model.generation, g);
        assert!(
            h.app
                .draft_msg
                .as_deref()
                .is_some_and(|m| m.contains("board.file") && m.contains("cannot read")),
            "{:?}",
            h.app.draft_msg
        );
    }

    #[test]
    fn the_waveform_tab_zooms_pans_and_places_a_cursor() {
        use sim_api::{Command, Input};
        let mut h = Harness::new();
        let client = h.app.model.client.clone();
        let run = |ps: u64| {
            client.command(Command::Run { duration_ps: ps }).unwrap();
        };
        // Switch 0 goes high at 2 ms of a 4 ms run.
        run(2_000_000_000);
        client
            .command(Command::Input(Input::SetSwitch { index: 0, on: true }))
            .unwrap();
        run(2_000_000_000);

        h.app.wave_ui.tab = true;
        h.app.model.waves.toggle("board.sw0");
        h.app.model.waves.toggle("uart1.tx");
        for _ in 0..3 {
            h.frame(vec![]);
        }
        let plot = h.app.wave_ui.plot.expect("the plot was drawn");
        assert!(h.app.model.waves.active);
        assert!(
            h.app
                .model
                .waves
                .catalogue
                .iter()
                .any(|s| s.name == "board.sw0"),
            "catalogue fetched"
        );
        let span0 = h.app.model.waves.span_ps;
        assert!(span0 >= 2_000_000_000, "fitted to the run: {span0}");
        let names: Vec<&str> = h
            .app
            .wave_ui
            .readouts
            .iter()
            .map(|r| r.0.as_str())
            .collect();
        assert_eq!(names, ["board.sw0", "uart1.tx"]);

        // A click at 25% and 75% of the window places the cursor and reads
        // the switch before and after it went high.
        let at = |frac: f32| pos2(plot.min.x + frac * plot.width(), plot.min.y + 40.0);
        h.click(at(0.25));
        h.frame(vec![]);
        let c1 = h.app.model.waves.cursor_ps.expect("cursor placed");
        assert!(c1 < 1_500_000_000 && c1 > 500_000_000, "cursor {c1}");
        assert_eq!(
            h.app.wave_ui.readouts[0].1, "0",
            "{:?}",
            h.app.wave_ui.readouts
        );
        h.click(at(0.75));
        h.frame(vec![]);
        let c2 = h.app.model.waves.cursor_ps.unwrap();
        assert!(c2 > c1);
        assert_eq!(
            h.app.wave_ui.readouts[0].1, "1",
            "{:?}",
            h.app.wave_ui.readouts
        );
        assert!(
            h.app.model.state.switches[0],
            "the cursor clicks sent no board input"
        );

        // The wheel zooms around the pointer: the time under it stays put.
        let p = at(0.5);
        let t_under = |h: &Harness, x: f32| {
            let w = &h.app.model.waves;
            w.from_ps as f64 + f64::from((x - plot.min.x) / plot.width()) * w.span_ps as f64
        };
        h.frame(vec![Event::PointerMoved(p)]);
        let before = t_under(&h, p.x);
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
        let span1 = h.app.model.waves.span_ps;
        assert!(span1 < span0 / 2, "zoomed in: {span0} -> {span1}");
        let after = t_under(&h, p.x);
        assert!(
            (after - before).abs() < span1 as f64 * 0.02,
            "time under the pointer moved {before} -> {after}"
        );
        // The data window followed the view and is no longer truncated.
        let d = h.app.model.waves.data.as_ref().unwrap();
        assert_eq!(d.from_ps, h.app.model.waves.from_ps);
        assert_eq!(d.to_ps, h.app.model.waves.to_ps());

        // Dragging to the right moves the window to earlier times.
        let from0 = h.app.model.waves.from_ps;
        assert!(from0 > 0);
        let a = at(0.4);
        h.frame(vec![Event::PointerMoved(a)]);
        h.press(a);
        h.frame(vec![Event::PointerMoved(a + vec2(30.0, 0.0))]);
        h.frame(vec![Event::PointerMoved(a + vec2(80.0, 0.0))]);
        h.release(a + vec2(80.0, 0.0));
        let from1 = h.app.model.waves.from_ps;
        assert!(from1 < from0, "panned earlier: {from0} -> {from1}");
        assert_eq!(h.app.model.waves.span_ps, span1, "a drag keeps the zoom");
        assert!(!h.app.model.waves.follow);

        // The drag did not move the cursor, and "fit" shows everything.
        assert_eq!(h.app.model.waves.cursor_ps, Some(c2));
        h.app.model.waves.fit();
        h.frame(vec![]);
        assert_eq!(h.app.model.waves.from_ps, 0);
        assert!(h.app.model.waves.span_ps >= span0);

        // Leaving the tab stops the fetching.
        h.app.wave_ui.tab = false;
        h.frame(vec![]);
        assert!(!h.app.model.waves.active);
    }

    /// Stands in for eframe's storage file.
    #[derive(Default)]
    struct MemStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MemStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn open_sections_are_saved_and_restored_through_storage() {
        let mut h = Harness::new();
        h.app.sections.set("source", false);
        h.app.sections.set("memory", true);
        let mut store = MemStorage::default();
        eframe::App::save(&mut h.app, &mut store);
        let saved = eframe::Storage::get_string(&store, SECTIONS_KEY).expect("state saved");

        let mut h2 = Harness::new();
        h2.app.restore_sections(&saved);
        assert!(!h2.app.sections.open_or("source", true));
        assert!(h2.app.sections.open_or("memory", false));
        // The restored panel still draws.
        h2.frame(vec![]);
    }

    #[test]
    fn the_speed_selector_reaches_the_engine_and_max_runs() {
        let mut h = Harness::new();
        assert_eq!(h.app.model.speed, crate::model::SPEED_REAL_TIME);
        h.app.model.speed = 4; // max
        h.app.model.play();
        h.frame(vec![]);
        assert!(h.app.model.status.playing, "play at max speed");
        h.app.model.pause();
        h.frame(vec![]);
        assert!(!h.app.model.status.playing);
    }

    #[test]
    fn the_memory_viewer_says_why_it_has_no_memory() {
        let h = Harness::new();
        assert!(!h.app.model.can_read_memory());
        assert_eq!(h.app.memory_summary(), "needs the qemu engine");
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

    // ---- the debugger offer and the restart (ADR-007, ADR-013, ADR-033) ----

    use sim_api::Engine;

    /// The demo engine reported as a QEMU run: `debug` is what the status says
    /// about the debugger. No registers are answered, as QEMU does without one.
    struct QemuLike {
        sim: sim_api::Simulator,
        debug: Option<sim_api::DebugStatus>,
    }

    impl Engine for QemuLike {
        fn def(&self) -> &sim_api::board::BoardDef {
            self.sim.def()
        }
        fn board(&self) -> &sim_api::board::BoardState {
            self.sim.board()
        }
        fn trace(&self) -> &sim_api::trace::TraceLog {
            self.sim.trace()
        }
        fn status(&self, playing: bool) -> sim_api::Status {
            let mut s = self.sim.status(playing);
            s.ps_backend = "qemu".to_string();
            s.debug = self.debug.clone();
            s
        }
        fn command(
            &mut self,
            cmd: sim_api::Command,
        ) -> Result<sim_api::Response, sim_api::ApiError> {
            self.sim.command(cmd)
        }
        fn query(
            &mut self,
            q: &sim_api::Query,
            playing: bool,
        ) -> Result<sim_api::Response, sim_api::ApiError> {
            if let sim_api::Query::Status = q {
                return Ok(sim_api::Response::Status(Box::new(self.status(playing))));
            }
            self.sim.query(q, playing)
        }
        fn run_for(&mut self, d: Ps) -> Result<(), sim_api::ApiError> {
            Engine::run_for(&mut self.sim, d)
        }
    }

    fn qemu_like(debug: Option<sim_api::DebugStatus>) -> Model {
        let sim = sim_api::Simulator::new(SimConfig::default()).unwrap();
        Model::new(SimClient::spawn_engine(Box::new(QemuLike { sim, debug }))).unwrap()
    }

    /// A draft of one of the examples (not built).
    fn example(name: &str) -> ProjectDraft {
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name)
            .join("project.toml");
        ProjectDraft::load(&file).unwrap()
    }

    fn elf_of(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/elf")
            .join(name)
    }

    /// The address range of an ELF's `.text` section.
    fn text_range(elf: &std::path::Path) -> (u64, u64) {
        use object::{Object, ObjectSection};
        let bytes = std::fs::read(elf).unwrap();
        let file = object::File::parse(&*bytes).unwrap();
        let text = file.section_by_name(".text").expect("a .text section");
        (text.address(), text.address() + text.size())
    }

    fn qemu_present() -> bool {
        if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
            eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
            return false;
        }
        std::env::var_os("ZYNQSIM_QEMU").is_some()
            || sim_api::locate_binary("qemu-system-aarch64").is_some()
    }

    /// Frames until the background build has finished, successfully or not.
    fn wait_for_build(h: &mut Harness, seconds: u64) {
        let start = Instant::now();
        while h.app.model.loading().is_some() {
            h.frame(vec![]);
            assert!(start.elapsed().as_secs() < seconds, "build did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        h.frame(vec![]);
    }

    /// Frames until the guest is stopped at a breakpoint.
    fn wait_for_stop(h: &mut Harness, seconds: u64) {
        let start = Instant::now();
        while !h.app.halted() {
            h.frame(vec![]);
            assert!(
                start.elapsed().as_secs() < seconds,
                "the guest did not stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        h.frame(vec![]);
    }

    #[test]
    fn the_debugger_offer_is_drawn_for_a_qemu_run_without_debug() {
        let mut h = Harness::with_model(qemu_like(None));
        h.app.draft = example("rust-hello");
        h.frame(vec![]);
        assert!(h.app.restart_offered());
        assert!(h.app.offer_drawn, "drawn where the registers would be");
        assert!(h.app.offer_button.is_some(), "with its restart button");
        assert!(
            h.app.native_possible(),
            "rust-hello is PS only, so the native button is offered too"
        );
        assert!(h.app.model.cpu_regs[0].is_none(), "nothing is asked for");

        // A project with PL cannot move to the native engine: no native button.
        h.app.draft = example("board-demo");
        assert!(!h.app.native_possible());
        assert_eq!(h.app.run_label(), "Play");

        // With the debugger attached there is nothing to offer, and the run
        // button says Continue while the guest is stopped.
        let stopped = sim_api::DebugStatus {
            stopped: true,
            ..Default::default()
        };
        let mut h = Harness::with_model(qemu_like(Some(stopped)));
        h.frame(vec![]);
        assert!(!h.app.restart_offered());
        assert!(!h.app.offer_drawn);
        assert!(h.app.under_debugger());
        assert_eq!(h.app.run_label(), "Continue");
    }

    #[test]
    fn the_restart_is_the_draft_with_the_debugger_and_a_start_breakpoint() {
        let mut h = Harness::with_model(qemu_like(None));
        h.app.draft = example("rust-hello");
        let on = h.app.debugger_project(true).unwrap();
        let d = on.debug.expect("a debug section");
        assert!(d.enabled);
        assert_eq!(
            d.breakpoints,
            vec!["main".to_string()],
            "rust_hello has main"
        );
        assert_eq!(
            on.cpu0,
            h.app.draft.to_project().unwrap().cpu0,
            "same firmware"
        );
        assert_eq!(
            on.backends,
            h.app.draft.to_project().unwrap().backends,
            "same backends"
        );
        let off = h.app.debugger_project(false).unwrap();
        assert!(
            off.debug.is_none_or(|d| !d.enabled),
            "rust-hello: no debugger"
        );
        // The draft, and so the project file, is unchanged.
        assert!(h.app.draft.base.debug.is_none());
        assert!(h.app.draft.to_project().unwrap().debug.is_none());
        // A draft with a problem is refused with the problem, not built.
        h.app.draft.cpu0.clear();
        assert!(h.app.debugger_project(true).is_err());
        // Nothing to stop at (the file is not an ELF): refused, not run to nowhere.
        h.app.draft = example("rust-hello");
        h.app.draft.cpu0 = "../../README.md".to_string();
        let err = h.app.debugger_project(true).unwrap_err();
        assert!(err.contains("breakpoint") || err.contains("cpu0"), "{err}");
    }

    #[test]
    fn the_restart_button_starts_the_debugger_build_of_the_current_project() {
        let mut h = Harness::with_model(qemu_like(None));
        h.app.draft = example("rust-hello");
        h.frame(vec![]);
        let at = h.app.offer_button.expect("the restart button is drawn");
        h.click(at.center());
        assert!(
            h.app.model.loading().is_some(),
            "the build of the debugger project is in the background"
        );
        // Whatever the machine has: the build ends with a swapped engine or an error.
        wait_for_build(&mut h, 240);
        assert!(h.app.model.generation > 0 || h.app.model.load_error.is_some());
    }

    #[test]
    fn the_native_engine_answers_registers_while_paused() {
        let mut h = Harness::new();
        h.app.draft = example("rust-hello");
        assert!(h.app.native_possible());
        h.app.use_native_engine();
        wait_for_engine(&mut h, 0);
        assert_eq!(h.app.model.status.ps_backend, "native");
        h.app.model.run_for(Ps::PER_MS);
        h.frame(vec![]);
        assert!(!h.app.model.status.playing, "paused: registers are read");
        let r = h.app.model.cpu_regs[0]
            .clone()
            .expect("the native engine answers at any time");
        let (lo, hi) = text_range(&elf_of("rust_hello.elf"));
        assert!(
            (lo..hi).contains(&u64::from(r.r[15])),
            "pc {:#x} is in .text {lo:#x}..{hi:#x}",
            r.r[15]
        );
        // Not a debugger stop: no restart offer, and no Continue label.
        assert!(!h.app.restart_offered());
        assert_eq!(h.app.run_label(), "Play");
    }

    #[test]
    fn the_qemu_restart_runs_rust_hello_to_main_and_shows_registers() {
        if !qemu_present() {
            eprintln!("SKIPPED: needs the Xilinx QEMU (qemu-system-aarch64)");
            return;
        }
        let mut h = Harness::new();
        h.app.draft = example("rust-hello");
        // The project as the window builds it: no debugger.
        h.app.build_draft("rust-hello");
        wait_for_engine(&mut h, 0);
        assert_eq!(h.app.model.status.ps_backend, "qemu");
        assert!(h.app.model.status.debug.is_none());
        assert!(h.app.model.cpu_regs[0].is_none());
        assert!(h.app.restart_offered());

        // One click's worth of work: rebuild with the debugger, run to the first stop.
        let g = h.app.model.generation;
        h.app.restart_under_debugger(true);
        wait_for_engine(&mut h, g);
        wait_for_stop(&mut h, 120);
        assert!(h.app.under_debugger());
        assert!(
            !h.app.restart_offered(),
            "the offer goes once the debugger is on"
        );

        let regs = h.app.model.cpu_regs[0]
            .clone()
            .expect("registers at the first stop");
        let elf = elf_of("rust_hello.elf");
        let (lo, hi) = text_range(&elf);
        assert!(
            (lo..hi).contains(&u64::from(regs.r[15])),
            "pc {:#x} is in .text {lo:#x}..{hi:#x}",
            regs.r[15]
        );
        assert_eq!(regs.pc_symbol.as_deref(), Some("main"), "stopped at main");
        assert!(
            h.app.model.source.view.is_none(),
            "rust_hello has no line information: the panel says so"
        );
        assert!(h.app.model.status.debug.as_ref().unwrap().stops >= 1);
    }

    #[test]
    fn the_qemu_restart_shows_the_source_line_of_rust_debug() {
        if !qemu_present() {
            eprintln!("SKIPPED: needs the Xilinx QEMU (qemu-system-aarch64)");
            return;
        }
        let mut h = Harness::new();
        h.app.draft = example("rust-debug");
        // The project's own [debug] breakpoint (main.rs:19) is kept by the restart.
        h.app.restart_under_debugger(true);
        wait_for_engine(&mut h, 0);
        wait_for_stop(&mut h, 120);
        let v = h
            .app
            .model
            .source
            .view
            .clone()
            .expect("the stop has a source line");
        assert!(v.file.ends_with("main.rs"), "{}", v.file);
        assert_eq!(v.line, 19, "stopped at the breakpoint on line 19");
        assert!(
            v.text.is_some(),
            "the firmware source was found: {:?}",
            v.note
        );
    }

    /// The demo engine with one Pmod sensor on JA that has no picture: its picture query fails,
    /// its settable quantity is answered (what the Pmod devices panel must still show).
    struct SensorNoPicture {
        sim: sim_api::Simulator,
    }

    impl Engine for SensorNoPicture {
        fn def(&self) -> &sim_api::board::BoardDef {
            self.sim.def()
        }
        fn board(&self) -> &sim_api::board::BoardState {
            self.sim.board()
        }
        fn trace(&self) -> &sim_api::trace::TraceLog {
            self.sim.trace()
        }
        fn status(&self, playing: bool) -> sim_api::Status {
            let mut s = self.sim.status(playing);
            s.devices = vec![sim_api::DeviceStatus {
                port: 0,
                kind: "sensor".into(),
                description: "a sensor with no picture".into(),
            }];
            s
        }
        fn command(
            &mut self,
            cmd: sim_api::Command,
        ) -> Result<sim_api::Response, sim_api::ApiError> {
            self.sim.command(cmd)
        }
        fn query(
            &mut self,
            q: &sim_api::Query,
            playing: bool,
        ) -> Result<sim_api::Response, sim_api::ApiError> {
            match q {
                sim_api::Query::Status => {
                    Ok(sim_api::Response::Status(Box::new(self.status(playing))))
                }
                sim_api::Query::DeviceImage { .. } => {
                    Err(sim_api::ApiError::unsupported("the sensor has no display"))
                }
                sim_api::Query::DeviceState { port: 0 } => Ok(sim_api::Response::DeviceState {
                    port: 0,
                    kind: "sensor".into(),
                    description: "a sensor with no picture".into(),
                    values: vec![sim_api::DeviceValueInfo {
                        name: "temperature".into(),
                        value: 23.5,
                        unit: "degC".into(),
                        min: -55.0,
                        max: 125.0,
                    }],
                    registers: Vec::new(),
                }),
                _ => self.sim.query(q, playing),
            }
        }
        fn run_for(&mut self, d: Ps) -> Result<(), sim_api::ApiError> {
            Engine::run_for(&mut self.sim, d)
        }
    }

    #[test]
    fn the_pmod_panel_draws_a_device_that_has_no_picture() {
        // Every device model draws a picture today, so this is the case the panel must still
        // handle: before the fix, a device without one lost its whole entry (name, state,
        // slider), because the panel only iterated the pictures.
        let sim = sim_api::Simulator::new(SimConfig::default()).unwrap();
        let model = Model::new(SimClient::spawn_engine(Box::new(SensorNoPicture { sim }))).unwrap();
        let mut h = Harness::with_model(model);
        h.frame(vec![]);
        assert!(h.app.model.device_images.is_empty(), "no picture");
        let rows = h.app.model.device_rows();
        assert_eq!(rows.len(), 1, "the device is listed");
        assert_eq!(rows[0].quantities.len(), 1, "with its quantity");
        assert_eq!(
            rows[0].quantities[0].1,
            crate::model::QuantityWidget::Slider,
            "drawn as a slider"
        );
    }
}
