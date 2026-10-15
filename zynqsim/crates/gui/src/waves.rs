//! The waveform viewer's state and behaviour without any drawing code, so it
//! is testable headless (ADR-014). The egui panel is `app/waves.rs`.

use crate::model::Model;
use sim_api::trace::wave_scene::format_value;
use sim_api::trace::waveform::{SignalInfo, SignalWave, Waveform};
use sim_api::{Query, Response, TraceFormat};
use std::path::Path;
use std::time::{Duration, Instant};

/// Value changes requested per signal for the visible window.
const MAX_CHANGES: usize = 4_000;
/// Least time between two refreshes of a live window (a changed window or
/// signal list is fetched at once).
const LIVE_REFRESH: Duration = Duration::from_millis(100);
/// Least time between two catalogue fetches.
const CATALOGUE_REFRESH: Duration = Duration::from_millis(500);
/// The shortest window the viewer zooms to, in picoseconds.
pub const MIN_SPAN_PS: u64 = 100;

/// Everything the waveform panel shows and does, except drawing.
#[derive(Default)]
pub struct WaveState {
    /// Signals shown, in display order. Survives a project reload.
    pub selected: Vec<String>,
    /// Every signal the trace holds, in name order.
    pub catalogue: Vec<SignalInfo>,
    /// The latest answer for the visible window.
    pub data: Option<Waveform>,
    /// Left edge of the visible window, in picoseconds.
    pub from_ps: u64,
    /// Width of the visible window, in picoseconds (0: not yet fitted).
    pub span_ps: u64,
    /// Keep the right edge on the newest time.
    pub follow: bool,
    /// The measurement cursor.
    pub cursor_ps: Option<u64>,
    /// Picker search text.
    pub filter: String,
    /// Time of the newest record, from the last catalogue.
    pub now_ps: u64,
    /// Set by the panel while it is on screen; nothing is fetched otherwise.
    pub active: bool,
    /// What the last export did (path or error), for the panel.
    pub notice: Option<String>,
    fetched: Option<Fetched>,
    catalogue_len: Option<usize>,
    catalogue_at: Option<Instant>,
    fetch_at: Option<Instant>,
}

#[derive(Clone, PartialEq, Eq)]
struct Fetched {
    names: Vec<String>,
    from: u64,
    to: u64,
    trace_len: usize,
}

impl WaveState {
    /// End of the visible window.
    pub fn to_ps(&self) -> u64 {
        self.from_ps.saturating_add(self.span_ps)
    }

    /// Whether `name` is shown.
    pub fn is_selected(&self, name: &str) -> bool {
        self.selected.iter().any(|n| n == name)
    }

    /// Shows or hides a signal.
    pub fn toggle(&mut self, name: &str) {
        if let Some(i) = self.selected.iter().position(|n| n == name) {
            self.selected.remove(i);
        } else {
            self.selected.push(name.to_string());
        }
    }

    /// Catalogue entries matching the picker's search text (case-insensitive,
    /// every space-separated word must occur in the name).
    pub fn filtered(&self) -> Vec<&SignalInfo> {
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.catalogue
            .iter()
            .filter(|s| {
                let n = s.name.to_lowercase();
                words.iter().all(|w| n.contains(w))
            })
            .collect()
    }

    /// Shows the whole run, from time 0 to the newest record.
    pub fn fit(&mut self) {
        self.from_ps = 0;
        self.span_ps = self.now_ps.max(MIN_SPAN_PS);
        self.follow = false;
    }

    /// Zooms by `factor` (above 1 shows less time) keeping the time under
    /// `anchor` (0..1 across the plot) fixed.
    pub fn zoom_at(&mut self, anchor: f64, factor: f64) {
        if self.span_ps == 0 {
            self.fit();
        }
        let anchor = anchor.clamp(0.0, 1.0);
        let t = self.from_ps as f64 + anchor * self.span_ps as f64;
        let max_span = (self.now_ps.max(MIN_SPAN_PS) as f64 * 2.0).max(1_000_000.0);
        let span = (self.span_ps as f64 / factor).clamp(MIN_SPAN_PS as f64, max_span);
        self.span_ps = span as u64;
        self.from_ps = (t - anchor * span).max(0.0) as u64;
        if self.follow {
            // Zooming in follow mode keeps following: the right edge stays
            // on the newest time.
            self.from_ps = self.now_ps.saturating_sub(self.span_ps);
        }
    }

    /// Moves the window by `delta_ps` (positive: later). Panning leaves
    /// follow mode.
    pub fn pan(&mut self, delta_ps: f64) {
        if self.span_ps == 0 {
            self.fit();
        }
        if delta_ps != 0.0 {
            self.follow = false;
        }
        let from = (self.from_ps as f64 + delta_ps).max(0.0);
        let latest = self.now_ps.max(self.span_ps) as f64;
        self.from_ps = from.min(latest) as u64;
    }

    /// The value text of `signal` at time `t` for the readout column.
    pub fn readout(&self, signal: &SignalWave, t: u64) -> String {
        match signal.value_at(t) {
            _ if !signal.found => "-".to_string(),
            Some(v) => {
                if signal.width > 1 {
                    format!("0x{}", format_value(v, signal.width))
                } else {
                    format_value(v, 1)
                }
            }
            None => "x".to_string(),
        }
    }
}

impl Model {
    /// Brings the catalogue and the visible window up to date. Cheap when
    /// nothing changed; does nothing while the panel is not on screen.
    pub fn refresh_waves(&mut self) {
        if !self.waves.active {
            return;
        }
        self.sync_waves(false);
    }

    /// Fetches what the panel needs now. `force` ignores the live-refresh
    /// throttle (used after the user changed the window).
    pub fn sync_waves(&mut self, force: bool) {
        let trace_len = self.status.trace_len;
        // Catalogue: when records were added, at most twice a second.
        let stale = self.waves.catalogue_len != Some(trace_len)
            && (force
                || self
                    .waves
                    .catalogue_at
                    .is_none_or(|t| t.elapsed() >= CATALOGUE_REFRESH));
        if stale || self.waves.catalogue_len.is_none() {
            if let Ok(Response::Signals {
                signals,
                now_ps,
                trace_len,
            }) = self.client.query(Query::SignalCatalogue)
            {
                self.waves.catalogue = signals;
                self.waves.now_ps = now_ps;
                self.waves.catalogue_len = Some(trace_len);
                self.waves.catalogue_at = Some(Instant::now());
            }
        }
        let now = self.status.now_ps.max(self.waves.now_ps);
        self.waves.now_ps = now;
        if self.waves.span_ps == 0 {
            self.waves.from_ps = 0;
            self.waves.span_ps = now.max(MIN_SPAN_PS);
        }
        if self.waves.follow {
            self.waves.from_ps = now.saturating_sub(self.waves.span_ps);
        }
        if self.waves.selected.is_empty() {
            self.waves.data = None;
            self.waves.fetched = None;
            return;
        }
        let want = Fetched {
            names: self.waves.selected.clone(),
            from: self.waves.from_ps,
            to: self.waves.to_ps(),
            trace_len,
        };
        if self.waves.fetched.as_ref() == Some(&want) {
            return;
        }
        // A live window (only new records) is throttled; a changed window or
        // signal list is not.
        let only_len_changed = self
            .waves
            .fetched
            .as_ref()
            .is_some_and(|f| f.names == want.names && f.from == want.from && f.to == want.to);
        if only_len_changed
            && !force
            && self
                .waves
                .fetch_at
                .is_some_and(|t| t.elapsed() < LIVE_REFRESH)
        {
            return;
        }
        match self.client.query(Query::Waveform {
            signals: want.names.clone(),
            from_ps: want.from,
            to_ps: want.to,
            max_changes: MAX_CHANGES,
        }) {
            Ok(Response::Waveform(w)) => {
                self.waves.data = Some(*w);
                self.waves.fetched = Some(want);
                self.waves.fetch_at = Some(Instant::now());
            }
            Ok(_) => {}
            Err(e) => self.last_error = Some(e.message),
        }
    }

    /// Writes the whole trace as VCD (the same export as the CLI and MCP).
    pub fn export_vcd(&mut self, path: &Path) -> Result<usize, String> {
        let r = match self.client.query(Query::TraceExport {
            format: TraceFormat::Vcd,
        }) {
            Ok(Response::Text(t)) => std::fs::write(path, &t)
                .map(|()| t.len())
                .map_err(|e| format!("cannot write {}: {e}", path.display())),
            Ok(other) => Err(format!("unexpected answer: {other:?}")),
            Err(e) => Err(e.message),
        };
        self.waves.notice = Some(match &r {
            Ok(n) => format!("wrote {} ({n} bytes)", path.display()),
            Err(e) => e.clone(),
        });
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_api::{Command, Input, SimClient, SimConfig};

    fn model() -> Model {
        let client = SimClient::spawn(SimConfig::default()).unwrap();
        Model::new(client).unwrap()
    }

    fn run(m: &mut Model, ps: u64) {
        m.client.command(Command::Run { duration_ps: ps }).unwrap();
        m.refresh();
    }

    #[test]
    fn zoom_keeps_the_time_under_the_pointer_and_pan_clamps() {
        let mut w = WaveState {
            now_ps: 1_000_000,
            ..WaveState::default()
        };
        w.fit();
        assert_eq!((w.from_ps, w.span_ps), (0, 1_000_000));
        // Zoom in 2x at the middle: the middle time stays under the middle.
        w.zoom_at(0.5, 2.0);
        assert_eq!((w.from_ps, w.span_ps), (250_000, 500_000));
        // Zoom in 4x at the left quarter.
        let t = w.from_ps as f64 + 0.25 * w.span_ps as f64;
        w.zoom_at(0.25, 4.0);
        assert_eq!(w.span_ps, 125_000);
        assert!((w.from_ps as f64 + 0.25 * w.span_ps as f64 - t).abs() < 2.0);
        // Never below the minimum span; never negative from.
        for _ in 0..40 {
            w.zoom_at(0.0, 4.0);
        }
        assert_eq!(w.span_ps, MIN_SPAN_PS);
        w.pan(-1e12);
        assert_eq!(w.from_ps, 0);
        w.pan(5e12);
        assert!(w.from_ps <= 1_000_000, "{}", w.from_ps);
        // Zooming out is bounded too.
        for _ in 0..40 {
            w.zoom_at(0.5, 0.25);
        }
        assert!(w.span_ps <= 2_000_000);
        // Panning leaves follow mode, a zero pan does not.
        w.follow = true;
        w.pan(0.0);
        assert!(w.follow);
        w.pan(10.0);
        assert!(!w.follow);
    }

    #[test]
    fn picker_filter_matches_every_word_ignoring_case() {
        let info = |n: &str| SignalInfo {
            name: n.to_string(),
            width: 1,
            changes: 1,
            first_ps: 0,
            last_ps: 0,
            derived: false,
        };
        let mut w = WaveState {
            catalogue: vec![info("board.led0"), info("board.sw0"), info("uart1.tx")],
            ..WaveState::default()
        };
        assert_eq!(w.filtered().len(), 3);
        w.filter = "LED".to_string();
        assert_eq!(w.filtered()[0].name, "board.led0");
        w.filter = "board 0".to_string();
        assert_eq!(w.filtered().len(), 2);
        w.filter = "nothing".to_string();
        assert!(w.filtered().is_empty());
        w.toggle("uart1.tx");
        assert!(w.is_selected("uart1.tx"));
        w.toggle("uart1.tx");
        assert!(w.selected.is_empty());
    }

    #[test]
    fn the_model_fetches_the_selected_window_only_while_active() {
        let mut m = model();
        m.client
            .command(Command::Input(Input::SetSwitch { index: 2, on: true }))
            .unwrap();
        run(&mut m, 2_000_000_000);
        m.waves.toggle("board.sw2");
        // Inactive: nothing is fetched.
        m.refresh();
        assert!(m.waves.data.is_none() && m.waves.catalogue.is_empty());

        m.waves.active = true;
        m.refresh();
        assert!(m.waves.catalogue.iter().any(|s| s.name == "board.sw2"));
        let d = m.waves.data.as_ref().expect("the window was fetched");
        assert_eq!(d.signals.len(), 1);
        assert_eq!(d.signals[0].name, "board.sw2");
        assert_eq!(d.signals[0].value_at(1_000_000_000), Some(1));
        assert_eq!((d.from_ps, d.to_ps), (0, m.waves.to_ps()));
        let sig = d.signals[0].clone();
        assert_eq!(m.waves.readout(&sig, 1_000_000_000), "1");

        // Zoom to a window before the switch was touched: the new window is
        // fetched at once and holds no change.
        m.waves.from_ps = 0;
        m.waves.span_ps = MIN_SPAN_PS;
        m.sync_waves(true);
        let d = m.waves.data.as_ref().unwrap();
        assert_eq!(d.to_ps, MIN_SPAN_PS);
        assert!(d.signals[0].changes.iter().all(|c| c.0 <= MIN_SPAN_PS));
        assert!(d.signals[0].in_window <= 2, "only the samples at time 0");

        // Follow mode tracks the newest time.
        m.waves.follow = true;
        m.waves.span_ps = 1_000_000_000;
        run(&mut m, 3_000_000_000);
        m.waves.active = true;
        m.sync_waves(true);
        assert_eq!(m.waves.to_ps(), m.status.now_ps.max(m.waves.now_ps));
    }

    #[test]
    fn the_signal_list_survives_an_engine_swap_and_export_writes_vcd() {
        let mut m = model();
        m.waves.toggle("board.led0");
        let g = m.generation;
        m.adopt(SimClient::spawn(SimConfig::default()).unwrap())
            .unwrap();
        assert_eq!(m.generation, g + 1);
        assert_eq!(m.waves.selected, ["board.led0"], "selection kept");

        run(&mut m, 1_000_000_000);
        let dir = std::env::temp_dir().join(format!("zynqsim-wave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.vcd");
        let n = m.export_vcd(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.len(), n);
        assert!(text.contains("$var wire 1") && text.contains("board.sw0"));
        assert!(m.waves.notice.as_ref().unwrap().contains("out.vcd"));
        // An unwritable path is reported, not swallowed.
        let e = m.export_vcd(&dir.join("no/such/dir/x.vcd")).unwrap_err();
        assert!(e.contains("cannot write"), "{e}");
        assert_eq!(m.waves.notice.as_deref(), Some(e.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
