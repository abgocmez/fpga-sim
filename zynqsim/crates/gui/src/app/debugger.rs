//! Registers and the Source panel need the debugger on QEMU (ADR-007). When the
//! engine runs without it, the side panel offers to restart the project under
//! the debugger (or on the native engine, when the project could run there).
//! The restart builds the project in memory: the project file is not written.

use super::App;
use super::shell::Action;
use crate::loader::PlKind;
use eframe::egui::{self, Button, Color32, RichText};
use sim_api::Project;

/// The status-bar note while the QEMU debugger is attached.
pub const DEBUGGER_NOTE: &str = "running under the debugger: timing is not reproducible";

impl App {
    /// Whether the engine is QEMU without the debugger: the registers and the
    /// source line are then unavailable, and the offer is shown.
    pub(super) fn restart_offered(&self) -> bool {
        self.model.status.ps_backend == "qemu" && self.model.status.debug.is_none()
    }

    /// Whether the engine is QEMU with the debugger attached (by `[debug]` or
    /// by a restart from the GUI).
    pub(super) fn under_debugger(&self) -> bool {
        self.model.status.ps_backend == "qemu" && self.model.status.debug.is_some()
    }

    /// Whether the guest is stopped at a breakpoint.
    pub(super) fn halted(&self) -> bool {
        self.model.status.debug.as_ref().is_some_and(|d| d.stopped)
    }

    /// The run button's label while not running: "Continue" at a breakpoint
    /// (it resumes and runs on to the next one), else "Play".
    pub(super) fn run_label(&self) -> &'static str {
        if self.halted() { "Continue" } else { "Play" }
    }

    /// Whether the draft could run on the native engine: PS only (no PL, no
    /// core 1), and the native engine's own checks pass for it. Thumb and VFP
    /// instructions are not checked here; the native engine reports them as a
    /// fault when it reaches one.
    pub(super) fn native_possible(&self) -> bool {
        if self.draft.pl != PlKind::None || !self.draft.cpu1.trim().is_empty() {
            return false;
        }
        let mut d = self.draft.clone();
        d.use_native = true;
        d.use_qemu = false;
        d.problems().is_empty()
    }

    /// The offer shown where the registers are missing: two lines of
    /// explanation and the buttons. Returns the button pressed, if any.
    pub(super) fn debugger_offer(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        self.offer_drawn = true;
        let busy = self.model.load.is_some();
        let native = self.native_possible();
        let mut picked = None;
        ui.label(
            RichText::new("Registers and source lines need the debugger, which this run lacks.")
                .small(),
        );
        ui.label(
            RichText::new("Restarting runs this project under it, stopped at main.")
                .small()
                .color(Color32::GRAY),
        );
        ui.horizontal_wrapped(|ui| {
            let restart = ui
                .add_enabled(!busy, Button::new("Restart under debugger"))
                .on_hover_text(
                    "Rebuild this project with [debug] enabled (the project file is not changed)",
                );
            self.offer_button = Some(restart.rect);
            if restart.clicked() {
                picked = Some(Action::RestartDebugger(true));
            }
            if native
                && ui
                    .add_enabled(!busy, Button::new("Use the native engine"))
                    .on_hover_text(
                        "The pure-Rust PS engine: registers at any time, no QEMU, no PL, core 0",
                    )
                    .clicked()
            {
                picked = Some(Action::UseNativeEngine);
            }
        });
        picked
    }

    /// The short form for the Source section: a single button.
    pub(super) fn debugger_button(&self, ui: &mut egui::Ui) -> Option<Action> {
        let busy = self.model.load.is_some();
        let clicked = ui
            .add_enabled(!busy, Button::new("Restart under debugger").small())
            .on_hover_text("Rebuild this project with the debugger; see CPU registers")
            .clicked();
        clicked.then_some(Action::RestartDebugger(true))
    }

    /// The project as the draft describes it, with the debugger switched on or
    /// off. Fails with the first validation problem.
    pub(super) fn debugger_project(&self, on: bool) -> Result<Project, String> {
        if let Some(first) = self.draft.problems().first() {
            return Err(first.clone());
        }
        let p = self.draft.to_project()?.with_debugger(&self.draft.dir, on);
        // Without a breakpoint the guest would run on and never stop.
        if on && p.debug.as_ref().is_none_or(|d| d.breakpoints.is_empty()) {
            return Err(
                "no breakpoint to stop at: the project has no ELF with a main or an entry point"
                    .to_string(),
            );
        }
        Ok(p)
    }

    /// Builds the current project with the debugger switched on or off, in the
    /// background. With the debugger on, the engine runs to its first stop
    /// (registers and the source line then appear).
    pub(super) fn restart_under_debugger(&mut self, on: bool) {
        if self.model.load.is_some() {
            return;
        }
        match self.debugger_project(on) {
            Ok(p) => {
                self.draft_msg = None;
                self.autoplay = on;
                let what = if on {
                    "debugger (restart)"
                } else {
                    "project without debugger"
                };
                self.model
                    .start_load(p, self.draft.dir.clone(), what.to_string());
            }
            Err(e) => self.model.last_error = Some(e),
        }
    }

    /// Switches the draft to the native engine and builds it.
    pub(super) fn use_native_engine(&mut self) {
        if self.model.load.is_some() || !self.native_possible() {
            return;
        }
        self.draft.use_native = true;
        self.draft.use_qemu = false;
        self.draft.pl = PlKind::None;
        self.draft.cpu1.clear();
        self.build_draft("native engine");
    }
}
