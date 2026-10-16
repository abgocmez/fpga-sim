//! Collapsible side-panel sections whose open state is remembered between runs.

use eframe::egui::{self, Color32, RichText};
use std::collections::BTreeMap;

/// The open state of each section the user has toggled. Sections never toggled
/// keep their default, which may depend on the simulation (CPU registers are
/// open while the guest is stopped).
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct SectionState {
    open: BTreeMap<String, bool>,
}

impl SectionState {
    /// Whether the section is open: the remembered state, else `default`.
    pub fn open_or(&self, key: &str, default: bool) -> bool {
        self.open.get(key).copied().unwrap_or(default)
    }

    /// Remembers that the section is open or closed.
    pub fn set(&mut self, key: &str, open: bool) {
        self.open.insert(key.to_string(), open);
    }

    /// The state as text, one `key=0|1` line per section.
    pub fn encode(&self) -> String {
        self.open
            .iter()
            .map(|(k, v)| format!("{k}={}\n", u8::from(*v)))
            .collect()
    }

    /// Reads [`SectionState::encode`]'s text. Lines that do not parse are
    /// ignored, so a damaged or newer file cannot break the panel.
    pub fn decode(text: &str) -> SectionState {
        let mut st = SectionState::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            match v.trim() {
                "1" => st.set(k, true),
                "0" => st.set(k, false),
                _ => {}
            }
        }
        st
    }
}

/// A collapsible section. While it is closed its header shows `summary` after
/// the title; a click toggles it and the new state is remembered.
pub fn section(
    ui: &mut egui::Ui,
    state: &mut SectionState,
    key: &str,
    title: &str,
    summary: &str,
    default_open: bool,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let open = state.open_or(key, default_open);
    let id = ui.make_persistent_id(("section", key));
    let mut cs =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
    cs.set_open(open);
    let header = cs.show_header(ui, |ui| {
        ui.strong(title);
        if !open {
            ui.label(RichText::new(summary).small().color(Color32::GRAY));
        }
    });
    let now_open = header.is_open();
    let _ = header.body(add_body);
    if now_open != open {
        state.set(key, now_open);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_state_round_trips_through_its_text() {
        let mut st = SectionState::default();
        assert!(st.open_or("cpu", true), "default when never toggled");
        st.set("cpu", false);
        st.set("memory", true);
        assert!(!st.open_or("cpu", true), "remembered over the default");
        let text = st.encode();
        assert_eq!(text, "cpu=0\nmemory=1\n");
        assert_eq!(SectionState::decode(&text), st);
    }

    #[test]
    fn damaged_lines_are_ignored() {
        let st = SectionState::decode("cpu=0\nnonsense\n=1\nbad name=1\nsource=maybe\nhdmi=1");
        assert!(!st.open_or("cpu", true));
        assert!(st.open_or("hdmi", false));
        assert!(
            st.open_or("source", true),
            "unparsed value keeps the default"
        );
        assert_eq!(st.encode(), "cpu=0\nhdmi=1\n");
    }
}
