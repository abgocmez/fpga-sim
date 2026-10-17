//! The top-level shell around the board: menu bar, status bar, keyboard
//! shortcuts, theme and UI scale, recent projects, examples, and the settings
//! remembered between runs. Every menu entry is an [`Action`] run by
//! [`App::run_action`], so the behaviour is testable without a window.

use super::App;
use crate::prefs::{self, Prefs, next_scale};
use eframe::egui::{
    self, Button, Color32, Key, Layout, Modifiers, RichText, ScrollArea, Sense, TextStyle,
    ThemePreference, Visuals,
};
use sim_api::Ps;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Which view the central area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Board2d,
    Board3d,
    Waves,
}

/// Everything a menu entry or shortcut can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    OpenProject,
    OpenFile(PathBuf),
    ClearRecent,
    RescanExamples,
    ProjectWindow,
    Quit,
    PlayPause,
    Step,
    Advance100Ms,
    Reset,
    SnapshotSave,
    SnapshotRestore,
    /// Rebuild the project with the debugger on or off, and run it to its first stop.
    RestartDebugger(bool),
    /// Build the project on the native PS engine instead of QEMU.
    UseNativeEngine,
    SetView(View),
    ToggleSide,
    ToggleConsole,
    ScaleUp,
    ScaleDown,
    ScaleReset,
    SetDark(bool),
    Shortcuts,
    About,
}

/// Shell state that is not part of the simulation.
pub struct Shell {
    /// Settings and history; saved to `persist_path` when it is set.
    pub prefs: Prefs,
    persist_path: Option<PathBuf>,
    dirty: bool,
    last_change: Instant,
    /// The theme and scale last pushed to the egui context.
    applied: Option<(bool, f32)>,
    pub show_side: bool,
    pub show_console: bool,
    pub shortcuts_open: bool,
    pub about_open: bool,
    /// `(name, project.toml)` of every example found at start-up.
    pub examples: Vec<(String, PathBuf)>,
    speed: SpeedMeter,
}

impl Shell {
    pub fn new(prefs: Prefs, persist_path: Option<PathBuf>, show_shortcuts: bool) -> Shell {
        Shell {
            prefs,
            persist_path,
            dirty: false,
            last_change: Instant::now(),
            applied: None,
            show_side: true,
            show_console: true,
            shortcuts_open: show_shortcuts,
            about_open: false,
            examples: prefs::find_examples_dir()
                .map(|d| prefs::list_examples(&d))
                .unwrap_or_default(),
            speed: SpeedMeter::default(),
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    /// Records that `file` was opened.
    pub fn note_recent(&mut self, file: &Path) {
        self.prefs.push_recent(file);
        self.touch();
    }

    /// Writes the settings now, if they are kept at all.
    pub fn save_now(&mut self) {
        if let Some(path) = &self.persist_path {
            if let Err(e) = self.prefs.save(path) {
                eprintln!(
                    "zynqsim-gui: cannot save settings to {}: {e}",
                    path.display()
                );
            }
        }
        self.dirty = false;
    }

    /// Pushes the theme and UI scale to egui when they changed.
    fn apply(&mut self, ctx: &egui::Context) {
        let want = (self.prefs.dark, self.prefs.ui_scale);
        if self.applied == Some(want) {
            return;
        }
        self.applied = Some(want);
        // Our own shortcuts handle Ctrl+= / - / 0; egui must not scale too.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_zoom_factor(want.1);
        ctx.set_theme(if want.0 {
            ThemePreference::Dark
        } else {
            ThemePreference::Light
        });
        ctx.style_mut(|s| {
            s.visuals = if want.0 {
                Visuals::dark()
            } else {
                Visuals::light()
            };
            // Floors for readability: the console, the trace and the
            // side panel text are monospace or small.
            for (style, min) in [
                (TextStyle::Small, 11.5),
                (TextStyle::Body, 13.0),
                (TextStyle::Button, 13.0),
                (TextStyle::Monospace, 13.0),
            ] {
                if let Some(font) = s.text_styles.get_mut(&style) {
                    font.size = font.size.max(min);
                }
            }
        });
    }
}

/// Measures simulated time per wall-clock second from the status samples.
#[derive(Default)]
struct SpeedMeter {
    samples: VecDeque<(f64, u64)>,
}

impl SpeedMeter {
    /// Keeps about two seconds of `(wall seconds, simulated picoseconds)`.
    fn push(&mut self, wall_s: f64, now_ps: u64) {
        self.samples.push_back((wall_s, now_ps));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|s| wall_s - s.0 > 2.0) {
            self.samples.pop_front();
        }
    }

    /// Simulated seconds per wall second over the kept samples, once they span
    /// at least a fifth of a second.
    fn rate(&self) -> Option<f64> {
        let (t0, p0) = *self.samples.front()?;
        let (t1, p1) = *self.samples.back()?;
        let dt = t1 - t0;
        (dt >= 0.2).then(|| p1.saturating_sub(p0) as f64 / 1e12 / dt)
    }
}

/// The name shown for a project file: its folder for `project.toml`.
pub fn project_label(file: &Path) -> String {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    if stem.as_deref() == Some("project") {
        if let Some(dir) = file.parent().and_then(Path::file_name) {
            return dir.to_string_lossy().into_owned();
        }
    }
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `(keys, what it does)` for the shortcuts window.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play / pause"),
    ("F6", "Step one quantum"),
    (
        "F5",
        "Reset the system (the seed box, when it holds a number, is the seed)",
    ),
    ("Ctrl+O", "Open a project"),
    ("Ctrl+=  Ctrl+-  Ctrl+0", "Larger, smaller, normal UI scale"),
    ("F1", "This window"),
    ("Ctrl+Q", "Quit"),
    (
        "Mouse, 2D view",
        "Click a switch to toggle it; hold a button with the pointer",
    ),
    (
        "Mouse, 3D view",
        "Drag to orbit, wheel to zoom, click a switch or button",
    ),
];

/// A menu entry with an optional shortcut hint; `true` when clicked (the menu
/// then closes).
fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let mut b = Button::new(label);
    if !shortcut.is_empty() {
        b = b.shortcut_text(shortcut);
    }
    let clicked = ui.add(b).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

impl App {
    /// Does one menu entry, shortcut or button press.
    pub(super) fn run_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenProject => self.open_project_dialog(),
            Action::OpenFile(file) => {
                if file.is_file() {
                    self.open_project_file(file);
                } else {
                    self.model.last_error = Some(format!("not found: {}", file.display()));
                    self.shell.prefs.forget(&file);
                    self.shell.touch();
                }
            }
            Action::ClearRecent => {
                self.shell.prefs.clear_recent();
                self.shell.touch();
            }
            Action::RescanExamples => {
                self.shell.examples = prefs::find_examples_dir()
                    .map(|d| prefs::list_examples(&d))
                    .unwrap_or_default();
            }
            Action::ProjectWindow => self.project_open = !self.project_open,
            Action::Quit => {
                self.shell.save_now();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::PlayPause => {
                if self.model.status.playing {
                    self.model.pause();
                } else {
                    self.model.play();
                }
            }
            Action::Step => self.model.step(),
            Action::Advance100Ms => self.model.run_for(100 * Ps::PER_MS),
            Action::Reset => self.model.reset(),
            Action::SnapshotSave => self.model.snapshot_save(),
            Action::SnapshotRestore => self.model.snapshot_restore(),
            Action::RestartDebugger(on) => self.restart_under_debugger(on),
            Action::UseNativeEngine => self.use_native_engine(),
            Action::SetView(v) => {
                self.view_3d = v == View::Board3d;
                self.wave_ui.tab = v == View::Waves;
            }
            Action::ToggleSide => self.shell.show_side = !self.shell.show_side,
            Action::ToggleConsole => self.shell.show_console = !self.shell.show_console,
            Action::ScaleUp => self.set_scale(next_scale(self.shell.prefs.ui_scale, true)),
            Action::ScaleDown => self.set_scale(next_scale(self.shell.prefs.ui_scale, false)),
            Action::ScaleReset => self.set_scale(1.0),
            Action::SetDark(dark) => {
                if self.shell.prefs.dark != dark {
                    self.shell.prefs.dark = dark;
                    self.shell.touch();
                }
            }
            Action::Shortcuts => self.shell.shortcuts_open = true,
            Action::About => self.shell.about_open = true,
        }
    }

    fn set_scale(&mut self, scale: f32) {
        if self.shell.prefs.ui_scale != scale {
            self.shell.prefs.ui_scale = scale;
            self.shell.touch();
        }
    }

    /// The view the central area shows now.
    pub(super) fn current_view(&self) -> View {
        if self.wave_ui.tab {
            View::Waves
        } else if self.view_3d {
            View::Board3d
        } else {
            View::Board2d
        }
    }

    /// Per-frame housekeeping: theme and scale, window size, speed sample and
    /// saving the settings.
    pub(super) fn shell_frame(&mut self, ctx: &egui::Context) {
        self.shell.apply(ctx);
        self.shell.speed.push(
            self.started.elapsed().as_secs_f64(),
            self.model.status.now_ps,
        );
        if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
            let size = [r.width(), r.height()];
            if size[0] > 0.0 && self.shell.prefs.window != Some(size) {
                self.shell.prefs.window = Some(size);
                self.shell.touch();
            }
        }
        // Saved after a short quiet time (a window drag changes the size on
        // every frame), and at once when the window is closed.
        let closing = ctx.input(|i| i.viewport().close_requested());
        if self.shell.dirty
            && (closing || self.shell.last_change.elapsed() >= Duration::from_millis(500))
        {
            self.shell.save_now();
        }
    }

    /// Keyboard shortcuts of the shell. Board keys are plain letters and digits,
    /// so Ctrl combinations and the function keys never collide with them.
    pub(super) fn shell_keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let take = |mods: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(mods, key));
        let mut acts = Vec::new();
        if take(Modifiers::COMMAND, Key::O) {
            acts.push(Action::OpenProject);
        }
        if take(Modifiers::COMMAND, Key::Q) {
            acts.push(Action::Quit);
        }
        if take(Modifiers::COMMAND, Key::Equals) || take(Modifiers::COMMAND, Key::Plus) {
            acts.push(Action::ScaleUp);
        }
        if take(Modifiers::COMMAND, Key::Minus) {
            acts.push(Action::ScaleDown);
        }
        if take(Modifiers::COMMAND, Key::Num0) {
            acts.push(Action::ScaleReset);
        }
        if !typing {
            if take(Modifiers::NONE, Key::Space) {
                acts.push(Action::PlayPause);
            }
            if take(Modifiers::NONE, Key::F5) {
                acts.push(Action::Reset);
            }
            if take(Modifiers::NONE, Key::F6) {
                acts.push(Action::Step);
            }
            if take(Modifiers::NONE, Key::F1) {
                acts.push(Action::Shortcuts);
            }
        }
        for a in acts {
            self.run_action(ctx, a);
        }
    }

    /// The menu bar. Returns the actions picked this frame; the caller runs them.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let mut acts: Vec<Action> = Vec::new();
        let playing = self.model.status.playing;
        let debugger_on = self.model.status.debug.is_some();
        let current = self.current_view();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if item(ui, "Open project…", "Ctrl+O") {
                    acts.push(Action::OpenProject);
                }
                ui.menu_button("Recent projects", |ui| {
                    let recent = self.shell.prefs.recent.clone();
                    if recent.is_empty() {
                        ui.add_enabled(false, Button::new("none yet"));
                    }
                    for p in &recent {
                        let label = format!("{}   ({})", project_label(p), p.display());
                        if ui.button(label).clicked() {
                            acts.push(Action::OpenFile(p.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!recent.is_empty(), Button::new("Clear list"))
                        .clicked()
                    {
                        acts.push(Action::ClearRecent);
                        ui.close();
                    }
                });
                ui.menu_button("Examples", |ui| {
                    if self.shell.examples.is_empty() {
                        ui.add_enabled(false, Button::new("no examples folder found"));
                    }
                    ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                        for (name, path) in &self.shell.examples {
                            if ui.button(name).clicked() {
                                acts.push(Action::OpenFile(path.clone()));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Rescan").clicked() {
                        acts.push(Action::RescanExamples);
                    }
                });
                ui.separator();
                if item(ui, "Project window…", "") {
                    acts.push(Action::ProjectWindow);
                }
                ui.separator();
                if item(ui, "Quit", "Ctrl+Q") {
                    acts.push(Action::Quit);
                }
            });
            ui.menu_button("Simulation", |ui| {
                let run = if playing { "Pause" } else { self.run_label() };
                if item(ui, run, "Space") {
                    acts.push(Action::PlayPause);
                }
                if item(ui, "Step", "F6") {
                    acts.push(Action::Step);
                }
                if item(ui, "Advance 100 ms", "") {
                    acts.push(Action::Advance100Ms);
                }
                if item(ui, "Reset", "F5") {
                    acts.push(Action::Reset);
                }
                ui.separator();
                if item(ui, "Save snapshot", "") {
                    acts.push(Action::SnapshotSave);
                }
                if item(ui, "Restore snapshot", "") {
                    acts.push(Action::SnapshotRestore);
                }
                ui.separator();
                // On when the QEMU engine has the debugger; a click rebuilds the
                // project with the other setting and runs it to its first stop.
                let qemu = self.model.status.ps_backend == "qemu";
                let busy = self.model.load.is_some();
                ui.add_enabled_ui(qemu && !busy, |ui| {
                    let mut on = debugger_on;
                    let resp = ui.checkbox(&mut on, "Debugger (restart)").on_hover_text(
                        "Rebuild this project with [debug] enabled and run it to the first \
                             stop (registers, source line). Timing is not reproducible under \
                             the debugger. The project file is not changed.",
                    );
                    if resp.clicked() {
                        acts.push(Action::RestartDebugger(on));
                        ui.close();
                    }
                });
            });
            ui.menu_button("View", |ui| {
                for (label, v) in [
                    ("2D board", View::Board2d),
                    ("3D board", View::Board3d),
                    ("Waveforms", View::Waves),
                ] {
                    if ui.radio(current == v, label).clicked() {
                        acts.push(Action::SetView(v));
                        ui.close();
                    }
                }
                ui.separator();
                let mut side = self.shell.show_side;
                if ui.checkbox(&mut side, "Side panel").clicked() {
                    acts.push(Action::ToggleSide);
                    ui.close();
                }
                let mut console = self.shell.show_console;
                if ui.checkbox(&mut console, "Console").clicked() {
                    acts.push(Action::ToggleConsole);
                    ui.close();
                }
                ui.separator();
                ui.add_enabled(
                    false,
                    Button::new(format!(
                        "UI scale {:.0}%",
                        self.shell.prefs.ui_scale * 100.0
                    )),
                );
                if item(ui, "Larger", "Ctrl+=") {
                    acts.push(Action::ScaleUp);
                }
                if item(ui, "Smaller", "Ctrl+-") {
                    acts.push(Action::ScaleDown);
                }
                if item(ui, "Normal size", "Ctrl+0") {
                    acts.push(Action::ScaleReset);
                }
                ui.separator();
                let mut dark = self.shell.prefs.dark;
                if ui.checkbox(&mut dark, "Dark theme").clicked() {
                    acts.push(Action::SetDark(dark));
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                if item(ui, "Keyboard shortcuts", "F1") {
                    acts.push(Action::Shortcuts);
                }
                if item(ui, "About ZynqSim", "") {
                    acts.push(Action::About);
                }
            });
        });
        acts
    }

    /// The bottom status line: time, run state, measured speed, project and
    /// backends, and the last error (click to clear it).
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let s = &self.model.status;
        let state = match &s.debug {
            Some(d) if d.stopped => match &d.reason {
                Some(r) => format!("halted at breakpoint ({r})"),
                None => "halted at breakpoint".to_string(),
            },
            _ if s.playing => "running".to_string(),
            _ => "paused".to_string(),
        };
        let speed = if !s.playing {
            "-".to_string()
        } else {
            match self.shell.speed.rate() {
                Some(r) => format!("{r:.2}x"),
                None => "measuring".to_string(),
            }
        };
        let project = match &self.draft.file {
            Some(f) => project_label(f),
            None => "built-in demo".to_string(),
        };
        let backends = format!("{} / {}", s.ps_backend, s.pl_backend);
        let error = self
            .model
            .load_error
            .clone()
            .or_else(|| self.model.last_error.clone())
            .map(|e| e.lines().next().unwrap_or("").to_string());
        // Right after the time, so it stays visible when the rest is cut short.
        let under = self.under_debugger();
        let note = if under {
            format!("{}   |   ", super::debugger::DEBUGGER_NOTE)
        } else {
            String::new()
        };
        let left = format!(
            "t = {}   |   {note}{state}   |   speed {speed}   |   {project}  [{backends}]",
            Ps(s.now_ps)
        );
        ui.horizontal(|ui| {
            // The left text is cut to the room the right side leaves, so a long
            // breakpoint message cannot run under the shortcut hint.
            let right_w = 280.0_f32.min(ui.available_width() * 0.5);
            let left_w = (ui.available_width() - right_w).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, ui.available_height()),
                Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(egui::Label::new(RichText::new(&left).monospace()).truncate())
                        .on_hover_text(&left);
                },
            );
            ui.with_layout(
                Layout::right_to_left(egui::Align::Center),
                |ui| match error {
                    Some(e) => {
                        let r = ui.add(
                            egui::Label::new(
                                RichText::new(e).color(Color32::from_rgb(240, 100, 90)),
                            )
                            .truncate()
                            .sense(Sense::click()),
                        );
                        if r.on_hover_text("click to clear").clicked() {
                            self.model.last_error = None;
                            self.model.load_error = None;
                        }
                    }
                    None => {
                        ui.label(RichText::new("F1: keyboard shortcuts").color(Color32::GRAY));
                    }
                },
            );
        });
    }

    /// The F1 window: the real bindings, the board's keys included.
    pub(super) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.shell.shortcuts_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("shortcut_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (keys, what) in SHORTCUTS {
                            ui.strong(*keys);
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.label(RichText::new(self.key_help()).monospace().small());
            });
        self.shell.shortcuts_open = open;
    }

    /// Help > About.
    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.shell.about_open {
            return;
        }
        let mut open = true;
        egui::Window::new("About ZynqSim")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.heading(format!("ZynqSim {}", env!("CARGO_PKG_VERSION")));
                ui.label("Desktop front end for the Zynq simulator: PS, PL and board.");
                ui.add_space(6.0);
                ui.label(
                    "What works, what is only drawn and what is verified by which test is \
                     listed in docs/STATUS.md. Its status words are verified, \
                     implemented-untested, stub and not-planned.",
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Nothing here has been compared with a real board.")
                        .color(Color32::GRAY),
                );
            });
        self.shell.about_open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Harness;
    use super::*;
    use eframe::egui::{Event, Key};

    /// A key press with the modifiers held in the same frame, as a window sends it.
    fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
        h.frame_with(
            vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
        h.frame(vec![]);
    }

    #[test]
    fn the_speed_meter_measures_simulated_per_wall_time() {
        let mut m = SpeedMeter::default();
        assert_eq!(m.rate(), None);
        m.push(0.0, 0);
        m.push(1.0, 1_000_000_000_000);
        assert!((m.rate().unwrap() - 1.0).abs() < 1e-9, "1 s per s");
        m.push(3.0, 1_500_000_000_000);
        // Only the last two seconds count.
        assert!((m.rate().unwrap() - 0.25).abs() < 1e-9, "{:?}", m.rate());
    }

    #[test]
    fn menu_actions_change_the_view_theme_and_scale() {
        let mut h = Harness::new();
        let ctx = h.ctx.clone();
        assert_eq!(h.app.current_view(), View::Board2d);
        h.app.run_action(&ctx, Action::SetView(View::Board3d));
        h.frame(vec![]);
        assert_eq!(h.app.current_view(), View::Board3d);
        h.app.run_action(&ctx, Action::SetView(View::Waves));
        h.frame(vec![]);
        assert!(h.app.wave_ui.tab && !h.app.view_3d);
        h.app.run_action(&ctx, Action::SetView(View::Board2d));
        h.frame(vec![]);
        assert_eq!(h.app.current_view(), View::Board2d);

        h.app.run_action(&ctx, Action::ToggleSide);
        h.app.run_action(&ctx, Action::ToggleConsole);
        assert!(!h.app.shell.show_side && !h.app.shell.show_console);
        h.frame(vec![]); // the hidden panels do not break the frame
        h.app.run_action(&ctx, Action::ToggleSide);
        assert!(h.app.shell.show_side);

        assert!(h.ctx.style().visuals.dark_mode, "dark is the default");
        h.app.run_action(&ctx, Action::SetDark(false));
        h.frame(vec![]);
        assert!(!h.ctx.style().visuals.dark_mode, "light after the toggle");
        assert!(!h.app.shell.prefs.dark);

        // egui applies a zoom factor at the start of the next pass.
        h.app.run_action(&ctx, Action::ScaleUp);
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(
            (h.ctx.zoom_factor() - 1.1).abs() < 1e-4,
            "{}",
            h.ctx.zoom_factor()
        );
        h.app.run_action(&ctx, Action::ScaleReset);
        h.frame(vec![]);
        h.frame(vec![]);
        assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-4);
        // Readability floors: the small and monospace text never shrink.
        let style = h.ctx.style();
        assert!(style.text_styles[&TextStyle::Small].size >= 11.5);
        assert!(style.text_styles[&TextStyle::Monospace].size >= 13.0);
    }

    #[test]
    fn keyboard_shortcuts_drive_the_shell() {
        let mut h = Harness::new();
        press(&mut h, Key::Num2, Modifiers::COMMAND); // Ctrl+2 is not a board key
        assert!(!h.app.model.state.switches[1]);
        press(&mut h, Key::Equals, Modifiers::COMMAND);
        assert!(
            (h.app.shell.prefs.ui_scale - 1.1).abs() < 1e-6,
            "Ctrl+= zooms in"
        );
        press(&mut h, Key::Minus, Modifiers::COMMAND);
        press(&mut h, Key::Num0, Modifiers::COMMAND);
        assert_eq!(h.app.shell.prefs.ui_scale, 1.0);

        press(&mut h, Key::F1, Modifiers::NONE);
        assert!(h.app.shell.shortcuts_open, "F1 opens the shortcuts");
        h.frame(vec![]);

        press(&mut h, Key::Space, Modifiers::NONE);
        assert!(
            h.app.model.status.playing || h.app.model.status.now_ps > 0,
            "Space plays"
        );
        h.app.model.pause();
        h.frame(vec![]);
        press(&mut h, Key::F6, Modifiers::NONE);
        press(&mut h, Key::F5, Modifiers::NONE); // reset: must not panic
        h.frame(vec![]);
        assert!(
            h.app.model.status.now_ps < 1_000_000_000_000,
            "reset went back to t=0"
        );
    }

    #[test]
    fn recent_projects_open_and_missing_ones_are_dropped() {
        let mut h = Harness::new();
        let ctx = h.ctx.clone();
        let missing = std::path::PathBuf::from("no/such/dir/project.toml");
        h.app.shell.prefs.push_recent(&missing);
        h.app.run_action(&ctx, Action::OpenFile(missing.clone()));
        assert!(
            h.app
                .model
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("not found")),
            "{:?}",
            h.app.model.last_error
        );
        let absolute_missing = std::path::absolute(&missing).unwrap();
        assert!(
            !h.app.shell.prefs.recent.contains(&absolute_missing),
            "the missing entry was forgotten: {:?}",
            h.app.shell.prefs.recent
        );
        h.app.run_action(&ctx, Action::ClearRecent);
        assert!(h.app.shell.prefs.recent.is_empty());
    }

    #[test]
    fn examples_from_the_repo_are_offered_and_open_in_the_background() {
        let mut h = Harness::new();
        let ctx = h.ctx.clone();
        assert!(
            h.app.shell.examples.iter().any(|(n, _)| n == "demo"),
            "the menu lists the repo's examples"
        );
        let path = h
            .app
            .shell
            .examples
            .iter()
            .find(|(n, _)| n == "demo")
            .map(|(_, p)| p.clone())
            .unwrap();
        let g = h.app.model.generation;
        h.app.run_action(&ctx, Action::OpenFile(path.clone()));
        let start = Instant::now();
        while h.app.model.generation == g {
            h.frame(vec![]);
            assert!(start.elapsed() < Duration::from_secs(20), "example loaded");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            h.app.shell.prefs.recent[0],
            std::path::absolute(&path).unwrap()
        );
    }

    #[test]
    fn the_status_line_and_project_labels() {
        assert_eq!(
            project_label(Path::new("examples/rust-debug/project.toml")),
            "rust-debug"
        );
        assert_eq!(project_label(Path::new("my-board.toml")), "my-board.toml");
        let mut h = Harness::new();
        h.app.shell.about_open = true;
        h.app.shell.shortcuts_open = true;
        h.frame(vec![]);
        assert!(
            h.app.shell.about_open && h.app.shell.shortcuts_open,
            "windows stay open"
        );
    }
}
