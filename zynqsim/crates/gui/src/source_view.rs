//! The "Source" panel: the file around the stopped pc, current line
//! highlighted (ADR-013). The state lives here, outside the drawing code, so
//! the model can be tested without a window.

use eframe::egui::{self, Color32, RichText};
use sim_api::{CpuRegs, Query, Response, SimClient};
use std::collections::HashMap;
use std::sync::Arc;

/// Lines shown on each side of the current one at first.
const DEFAULT_CONTEXT: usize = 6;
/// Most lines of context on each side.
const MAX_CONTEXT: usize = 40;

/// Where a stopped core is in the source.
#[derive(Clone, Debug)]
pub struct SourceView {
    /// The core whose pc this is.
    pub core: u8,
    /// File as the ELF's line table names it.
    pub file: String,
    /// 1-based current line.
    pub line: u32,
    /// Where the file was found, when it was.
    pub resolved: Option<String>,
    /// The whole file, when readable.
    pub text: Option<Arc<Vec<String>>>,
    /// Why there is no text.
    pub note: Option<String>,
}

impl SourceView {
    /// `(line number, text)` of up to `context` lines each side of the
    /// current one; empty when the file is not available.
    pub fn window(&self, context: usize) -> Vec<(u32, &str)> {
        let Some(text) = &self.text else {
            return Vec::new();
        };
        let cur = self.line as usize;
        let from = cur.saturating_sub(context).max(1);
        let to = (cur + context).min(text.len());
        (from..=to)
            .filter_map(|n| text.get(n - 1).map(|l| (n as u32, l.as_str())))
            .collect()
    }
}

type FileResult = Result<(String, Arc<Vec<String>>), String>;

/// Follows the stop and keeps what the panel needs.
#[derive(Debug)]
pub struct SourceTracker {
    /// Lines of context on each side of the current line.
    pub context: usize,
    /// Present while a core is stopped on a line the ELF has information for.
    pub view: Option<SourceView>,
    files: HashMap<String, FileResult>,
}

impl Default for SourceTracker {
    fn default() -> Self {
        SourceTracker {
            context: DEFAULT_CONTEXT,
            view: None,
            files: HashMap::new(),
        }
    }
}

impl SourceTracker {
    /// Updates [`SourceTracker::view`] from the registers of a refresh; reads a
    /// source file once and keeps it.
    pub fn update(&mut self, client: &SimClient, regs: &[Option<CpuRegs>; 2]) {
        let found = regs.iter().enumerate().find_map(|(core, r)| {
            let s = r.as_ref()?.pc_source.as_ref()?;
            Some((core as u8, s.file.clone(), s.line))
        });
        let Some((core, file, line)) = found else {
            self.view = None;
            return;
        };
        let entry = self.files.entry(file.clone()).or_insert_with(|| {
            match client.query(Query::SourceFile { path: file.clone() }) {
                Ok(Response::SourceFile { text, resolved, .. }) => Ok((
                    resolved,
                    Arc::new(text.lines().map(str::to_string).collect()),
                )),
                Ok(r) => Err(format!("unexpected response {r:?}")),
                Err(e) => Err(e.message),
            }
        });
        let (resolved, text, note) = match entry {
            Ok((p, t)) => (Some(p.clone()), Some(Arc::clone(t)), None),
            Err(why) => (None, None, Some(why.clone())),
        };
        self.view = Some(SourceView {
            core,
            file,
            line,
            resolved,
            text,
            note,
        });
    }

    /// Shows more or fewer lines.
    pub fn set_context(&mut self, context: usize) {
        self.context = context.clamp(1, MAX_CONTEXT);
    }
}

/// The buttons the panel drew, for tests that click them.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelRects {
    /// "more" button.
    pub more: Option<egui::Rect>,
    /// "less" button.
    pub less: Option<egui::Rect>,
}

/// Draws the panel body (inside a collapsing header or a window). `stopped_at`
/// describes the pc of a stopped core, when there is one: then the panel says
/// that the ELF has no line for it instead of that the guest is not stopped.
pub fn show(
    ui: &mut egui::Ui,
    tracker: &mut SourceTracker,
    stopped_at: Option<&str>,
) -> PanelRects {
    let mut rects = PanelRects::default();
    let Some(v) = tracker.view.clone() else {
        let text = match stopped_at {
            Some(pc) => format!(
                "No line information for {pc}: its ELF has no debug line table for this pc."
            ),
            None => "No source line: the guest is not stopped at a breakpoint, or its ELF was \
                     built without debug info."
                .to_string(),
        };
        ui.label(RichText::new(text).color(Color32::GRAY).small());
        return rects;
    };
    ui.label(RichText::new(format!("core {}  {}:{}", v.core, v.file, v.line)).strong());
    ui.horizontal(|ui| {
        ui.label(RichText::new("context").small());
        let less = ui.small_button("-");
        let more = ui.small_button("+");
        rects.less = Some(less.rect);
        rects.more = Some(more.rect);
        if less.clicked() {
            let c = tracker.context.saturating_sub(2);
            tracker.set_context(c);
        }
        if more.clicked() {
            let c = tracker.context + 2;
            tracker.set_context(c);
        }
    });
    let window = v.window(tracker.context);
    if window.is_empty() {
        ui.label(
            RichText::new(v.note.as_deref().unwrap_or("source file is empty"))
                .color(Color32::GRAY)
                .small(),
        );
        return rects;
    }
    let width = window.last().map_or(1, |(n, _)| n.to_string().len());
    // Long lines scroll sideways instead of wrapping under the line numbers.
    egui::ScrollArea::horizontal()
        .id_salt("source_lines")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (n, text) in window {
                let current = n == v.line;
                let marker = if current { "=>" } else { "  " };
                let mut t = RichText::new(format!("{marker} {n:>width$}  {text}")).monospace();
                if current {
                    t = t.strong().background_color(Color32::from_rgb(120, 90, 0));
                }
                ui.add(egui::Label::new(t).wrap_mode(egui::TextWrapMode::Extend));
            }
        });
    if let Some(p) = &v.resolved {
        ui.label(RichText::new(p).small().color(Color32::GRAY));
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use eframe::egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
    use sim_api::{Ps, SimConfig, SourceRef};

    fn view(lines: usize, line: u32) -> SourceView {
        SourceView {
            core: 0,
            file: "a/b.rs".into(),
            line,
            resolved: Some("/x/a/b.rs".into()),
            text: Some(Arc::new((1..=lines).map(|i| format!("line {i}")).collect())),
            note: None,
        }
    }

    #[test]
    fn the_window_is_clipped_to_the_file() {
        let v = view(20, 3);
        let w = v.window(6);
        assert_eq!(w.first().map(|l| l.0), Some(1));
        assert_eq!(w.last().map(|l| l.0), Some(9));
        assert_eq!(w[2], (3, "line 3"));
        let v = view(20, 19);
        assert_eq!(v.window(4).last().map(|l| l.0), Some(20));
        let none = SourceView {
            text: None,
            note: Some("not found".into()),
            ..view(1, 1)
        };
        assert!(none.window(5).is_empty());
    }

    /// Runs one frame of the panel and returns every text it drew plus the
    /// button rectangles.
    fn frame(
        ctx: &egui::Context,
        t: &mut SourceTracker,
        events: Vec<Event>,
        time: f64,
    ) -> (Vec<String>, PanelRects) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            events,
            time: Some(time),
            ..Default::default()
        };
        let mut rects = PanelRects::default();
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                rects = show(ui, t, None);
            });
        });
        let mut texts = Vec::new();
        for c in &out.shapes {
            collect(&c.shape, &mut texts);
        }
        (texts, rects)
    }

    fn collect(s: &egui::epaint::Shape, out: &mut Vec<String>) {
        match s {
            egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }

    #[test]
    fn the_panel_draws_the_window_and_the_buttons_change_it() {
        let ctx = egui::Context::default();
        let mut t = SourceTracker {
            view: Some(view(100, 50)),
            ..Default::default()
        };
        // The first frame lays out, the second is the one to read.
        frame(&ctx, &mut t, vec![], 0.0);
        let (texts2, rects) = frame(&ctx, &mut t, vec![], 0.05);
        // Current line marked, 6 lines of context each side.
        assert!(
            texts2
                .iter()
                .any(|s| s.starts_with("=>") && s.ends_with("line 50"))
        );
        assert!(texts2.iter().any(|s| s.ends_with("line 44")));
        assert!(texts2.iter().any(|s| s.ends_with("line 56")));
        assert!(!texts2.iter().any(|s| s.ends_with("line 57")));
        assert!(texts2.iter().any(|s| s.contains("a/b.rs:50")));

        // Click "+": two more lines each side.
        let p = rects.more.expect("more button drawn").center();
        let click = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&ctx, &mut t, vec![Event::PointerMoved(p)], 0.10);
        frame(&ctx, &mut t, vec![click(true)], 0.15);
        frame(&ctx, &mut t, vec![click(false)], 0.20);
        assert_eq!(t.context, 8, "the button grew the window");
        let (texts, _) = frame(&ctx, &mut t, vec![], 0.25);
        assert!(texts.iter().any(|s| s.ends_with("line 58")));

        // A stop with no line information explains itself.
        t.view = None;
        let (texts, _) = frame(&ctx, &mut t, vec![], 0.30);
        assert!(
            texts.iter().any(|s| s.contains("No source line")),
            "{texts:?}"
        );
    }

    fn qemu_present() -> bool {
        if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
            eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
            return false;
        }
        std::env::var_os("ZYNQSIM_QEMU").is_some()
            || sim_api::locate_binary("qemu-system-aarch64").is_some()
    }

    #[test]
    fn the_model_follows_a_stop_to_its_source_line() {
        if !qemu_present() {
            eprintln!("SKIPPED: needs the Xilinx QEMU");
            return;
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = std::fs::read_to_string(root.join("firmware/debug_demo/src/main.rs")).unwrap();
        let step = src
            .lines()
            .position(|l| l.contains("MARK:collatz-step"))
            .unwrap() as u32
            + 1;
        let draft =
            crate::loader::ProjectDraft::load(&root.join("examples/rust-debug/project.toml"))
                .unwrap();
        let mut m = Model::new(SimClient::spawn(SimConfig::default()).unwrap()).unwrap();
        m.start_load(draft.to_project().unwrap(), draft.dir.clone(), "src".into());
        let start = std::time::Instant::now();
        while m.generation == 0 {
            m.refresh();
            assert!(m.load_error.is_none(), "{:?}", m.load_error);
            assert!(start.elapsed().as_secs() < 60, "load did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(m.source.view.is_none(), "nothing before the first stop");
        m.run_for(Ps::PER_MS);
        m.refresh();
        let v = m.source.view.clone().expect("a source view while stopped");
        assert_eq!((v.core, v.line), (0, step));
        assert!(v.file.ends_with("debug_demo/src/main.rs"), "{}", v.file);
        let r = m.cpu_regs[0].as_ref().unwrap();
        assert_eq!(
            r.pc_source,
            Some(SourceRef {
                file: v.file.clone(),
                line: step
            })
        );
        let w = v.window(m.source.context);
        let cur = w.iter().find(|(n, _)| *n == step).expect("current line");
        assert!(cur.1.contains("MARK:collatz-step"), "{}", cur.1);
        assert!(w.len() > 5, "context around it");

        // The panel draws it, current line marked.
        let ctx = egui::Context::default();
        frame(&ctx, &mut m.source, vec![], 0.0);
        let (texts2, _) = frame(&ctx, &mut m.source, vec![], 0.05);
        assert!(
            texts2
                .iter()
                .any(|s| s.starts_with("=>") && s.contains("MARK:collatz-step")),
            "{texts2:?}"
        );
    }
}
