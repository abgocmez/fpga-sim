//! The project window: load PS firmware and PL design separately, set the
//! media sources, then build and run.

use super::{App, dialogs};
use crate::loader::{PlKind, ProjectDraft};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;

impl App {
    /// Starts building the engine described by the draft.
    pub(super) fn build_draft(&mut self, what: &str) {
        match self.draft.to_project() {
            Err(e) => self.draft_msg = Some(e),
            Ok(p) => {
                let problems = self.draft.problems();
                if let Some(first) = problems.first() {
                    self.draft_msg = Some(first.clone());
                    return;
                }
                self.draft_msg = None;
                self.autoplay = self.draft.uart_tcp;
                self.model
                    .start_load(p, self.draft.dir.clone(), what.to_string());
            }
        }
    }

    /// Asks for a project file and builds it.
    pub(super) fn open_project_dialog(&mut self) {
        let dir = self.draft.dir.clone();
        let Some(file) = dialogs::pick_file(
            "Open a ZynqSim project",
            &dir,
            &[("project", &["toml"]), ("all files", &["*"])],
        ) else {
            if !dialogs::AVAILABLE {
                self.draft_msg = Some(
                    "this build has no file dialogs: type the project path in the window"
                        .to_string(),
                );
                self.project_open = true;
            }
            return;
        };
        self.open_project_file(file);
    }

    /// Loads `file` into the draft and builds it.
    pub(super) fn open_project_file(&mut self, file: PathBuf) {
        match ProjectDraft::load(&file) {
            Ok(d) => {
                self.shell.note_recent(&file);
                self.draft = d;
                let name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.build_draft(&format!("project {name}"));
            }
            Err(e) => {
                self.draft_msg = Some(e);
                self.project_open = true;
            }
        }
    }

    fn browse_into(
        draft_dir: &std::path::Path,
        title: &str,
        filters: &[(&str, &[&str])],
        target: &mut String,
        append: bool,
    ) {
        for f in dialogs::pick_files(title, draft_dir, filters) {
            let rel = dialogs::relative_to(draft_dir, &f);
            if append {
                if !target.is_empty() && !target.ends_with('\n') {
                    target.push('\n');
                }
                target.push_str(&rel);
            } else {
                *target = rel;
                break;
            }
        }
    }

    /// The project window.
    pub(super) fn project_window(&mut self, ctx: &egui::Context) {
        let mut open = self.project_open;
        let mut build: Option<&'static str> = None;
        egui::Window::new("Project: PS firmware and PL design")
            .open(&mut open)
            .default_width(520.0)
            // Opens below the view tabs instead of covering them.
            .default_pos([330.0, 100.0])
            .show(ctx, |ui| {
                let dir = self.draft.dir.clone();
                ui.label(
                    RichText::new(match &self.draft.file {
                        Some(f) => format!("from {}", f.display()),
                        None => "not saved yet".to_string(),
                    })
                    .small()
                    .color(Color32::GRAY),
                );

                ui.separator();
                ui.heading("Board");
                ui.horizontal(|ui| {
                    let current = if self.draft.board_file.trim().is_empty() {
                        self.draft.board_name.clone()
                    } else {
                        "custom file".to_string()
                    };
                    egui::ComboBox::from_id_salt("board_picker")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for name in sim_api::board::BoardDef::builtin_names() {
                                if ui
                                    .selectable_label(
                                        self.draft.board_file.trim().is_empty()
                                            && self.draft.board_name == name,
                                        name,
                                    )
                                    .clicked()
                                {
                                    self.draft.board_name = name.to_string();
                                    self.draft.board_file.clear();
                                }
                            }
                            if ui
                                .selectable_label(
                                    !self.draft.board_file.trim().is_empty(),
                                    "custom file…",
                                )
                                .clicked()
                                && self.draft.board_file.trim().is_empty()
                            {
                                self.draft.board_file = "my-board.toml".to_string();
                            }
                        });
                    if !self.draft.board_file.trim().is_empty() {
                        ui.add_sized(
                            [240.0, 20.0],
                            egui::TextEdit::singleline(&mut self.draft.board_file),
                        );
                        if ui.button("Browse…").clicked() {
                            Self::browse_into(
                                &dir,
                                "Board description (TOML)",
                                &[("board", &["toml"]), ("all files", &["*"])],
                                &mut self.draft.board_file,
                                false,
                            );
                        }
                    }
                });
                ui.label(
                    RichText::new(
                        "A board changes the switches, buttons and LEDs the window shows. \
                         Build to apply it.",
                    )
                    .small()
                    .color(Color32::GRAY),
                );

                ui.separator();
                ui.heading("PS (Cortex-A9 firmware)");
                ui.checkbox(
                    &mut self.draft.use_qemu,
                    "run firmware on QEMU (otherwise the built-in demo engine)",
                );
                ui.add_enabled_ui(self.draft.use_qemu, |ui| {
                    egui::Grid::new("ps_grid").num_columns(3).show(ui, |ui| {
                        ui.label("core 0 ELF");
                        ui.add_sized(
                            [300.0, 20.0],
                            egui::TextEdit::singleline(&mut self.draft.cpu0),
                        );
                        if ui.button("Browse…").clicked() {
                            Self::browse_into(
                                &dir,
                                "Core 0 firmware (ELF)",
                                &[("ELF", &["elf", "axf"]), ("all files", &["*"])],
                                &mut self.draft.cpu0,
                                false,
                            );
                        }
                        ui.end_row();
                        ui.label("core 1 ELF");
                        ui.add_sized(
                            [300.0, 20.0],
                            egui::TextEdit::singleline(&mut self.draft.cpu1).hint_text("optional"),
                        );
                        if ui.button("Browse…").clicked() {
                            Self::browse_into(
                                &dir,
                                "Core 1 firmware (ELF)",
                                &[("ELF", &["elf", "axf"]), ("all files", &["*"])],
                                &mut self.draft.cpu1,
                                false,
                            );
                        }
                        ui.end_row();
                    });
                    ui.checkbox(
                        &mut self.draft.uart_tcp,
                        "interactive UART1 (TCP): the console can type to the firmware, \
                         and the simulation starts running at once",
                    );
                });

                ui.separator();
                ui.heading("PL (programmable logic)");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.draft.pl, PlKind::None, "none");
                    ui.selectable_value(&mut self.draft.pl, PlKind::Native, "native Rust models");
                    ui.selectable_value(&mut self.draft.pl, PlKind::Xsim, "RTL in Vivado XSim");
                });
                match self.draft.pl {
                    PlKind::Xsim => {
                        ui.label("RTL sources, one per line (Verilog / SystemVerilog):");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.draft.rtl_files)
                                .desired_rows(4)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Add files…").clicked() {
                                Self::browse_into(
                                    &dir,
                                    "RTL sources",
                                    &[("RTL", &["v", "sv", "vh", "svh"]), ("all files", &["*"])],
                                    &mut self.draft.rtl_files,
                                    true,
                                );
                            }
                            if ui.button("Clear").clicked() {
                                self.draft.rtl_files.clear();
                            }
                            ui.label("top module");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.draft.rtl_top)
                                    .desired_width(160.0)
                                    .hint_text("auto"),
                            );
                        });
                    }
                    PlKind::Native => {
                        ui.label(
                            RichText::new(
                                "Native models and their addresses come from the project's \
                                 [native] and [connect] tables (edit the file to change them).",
                            )
                            .small()
                            .color(Color32::GRAY),
                        );
                    }
                    PlKind::None => {}
                }

                ui.separator();
                ui.heading("Board media");
                egui::Grid::new("media_grid").num_columns(3).show(ui, |ui| {
                    ui.label("HDMI RX picture");
                    ui.add_sized(
                        [300.0, 20.0],
                        egui::TextEdit::singleline(&mut self.draft.video_in)
                            .hint_text("bars, gradient:640x480 or a PNG"),
                    );
                    if ui.button("Browse…").clicked() {
                        Self::browse_into(
                            &dir,
                            "HDMI RX picture",
                            &[("PNG", &["png"])],
                            &mut self.draft.video_in,
                            false,
                        );
                    }
                    ui.end_row();
                    ui.label("LINE IN audio");
                    ui.add_sized(
                        [300.0, 20.0],
                        egui::TextEdit::singleline(&mut self.draft.audio_in)
                            .hint_text("silence, tone:440:0.5 or a WAV"),
                    );
                    if ui.button("Browse…").clicked() {
                        Self::browse_into(
                            &dir,
                            "LINE IN audio",
                            &[("WAV", &["wav"])],
                            &mut self.draft.audio_in,
                            false,
                        );
                    }
                    ui.end_row();
                    ui.label("quantum (ns)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.quantum_ns).desired_width(80.0),
                    );
                    ui.end_row();
                });

                ui.separator();
                let problems = self.draft.problems();
                for p in &problems {
                    ui.colored_label(Color32::from_rgb(240, 120, 90), p);
                }
                if let Some(m) = &self.draft_msg {
                    ui.colored_label(Color32::from_rgb(240, 120, 90), m);
                }
                if let Some(e) = &self.model.load_error {
                    ui.colored_label(Color32::from_rgb(240, 100, 90), e);
                }
                let busy = self.model.load.is_some();
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!busy && problems.is_empty(), |ui| {
                        if ui
                            .button("Build and run")
                            .on_hover_text("Start a fresh simulation from these settings")
                            .clicked()
                        {
                            build = Some("project");
                        }
                    });
                    if ui
                        .button("Load PS firmware…")
                        .on_hover_text(
                            "Pick a new ELF for the CPU and restart; the PL design is kept",
                        )
                        .clicked()
                    {
                        if let Some(f) = dialogs::pick_file(
                            "PS firmware (ELF)",
                            &dir,
                            &[("ELF", &["elf", "axf"]), ("all files", &["*"])],
                        ) {
                            self.draft.use_qemu = true;
                            self.draft.cpu0 = dialogs::relative_to(&dir, &f);
                            build = Some("PS firmware");
                        }
                    }
                    if ui
                        .button("Load PL design…")
                        .on_hover_text(
                            "Pick RTL files for the PL and restart; the PS firmware is kept",
                        )
                        .clicked()
                    {
                        let files = dialogs::pick_files(
                            "PL design (Verilog / SystemVerilog)",
                            &dir,
                            &[("RTL", &["v", "sv"]), ("all files", &["*"])],
                        );
                        if !files.is_empty() {
                            self.draft.pl = PlKind::Xsim;
                            self.draft.rtl_files = files
                                .iter()
                                .map(|f| dialogs::relative_to(&dir, f))
                                .collect::<Vec<_>>()
                                .join("\n");
                            build = Some("PL design");
                        }
                    }
                    if ui.button("Save project.toml…").clicked() {
                        let name = self
                            .draft
                            .file
                            .as_ref()
                            .and_then(|f| f.file_name())
                            .map_or_else(
                                || "project.toml".to_string(),
                                |n| n.to_string_lossy().into_owned(),
                            );
                        if let Some(path) = dialogs::save_file("Save project", &dir, &name) {
                            match self.draft.to_toml() {
                                Ok(text) => match std::fs::write(&path, text) {
                                    Ok(()) => {
                                        self.draft.dir = path
                                            .parent()
                                            .map(PathBuf::from)
                                            .unwrap_or_else(|| PathBuf::from("."));
                                        self.draft.file = Some(path);
                                        self.draft_msg = None;
                                    }
                                    Err(e) => self.draft_msg = Some(format!("cannot save: {e}")),
                                },
                                Err(e) => self.draft_msg = Some(e),
                            }
                        }
                    }
                    if ui.button("Open project…").clicked() {
                        self.open_requested = true;
                    }
                });
                if !dialogs::AVAILABLE {
                    ui.label(
                        RichText::new(
                            "Built without file dialogs: type the paths above, then Build and run.",
                        )
                        .small()
                        .color(Color32::GRAY),
                    );
                }
            });
        self.project_open = open;
        if let Some(what) = build {
            self.build_draft(what);
        }
        if self.open_requested {
            self.open_requested = false;
            self.open_project_dialog();
        }
    }
}
