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
/// Signals a project opens with, at most.
pub const DEFAULT_SIGNALS: usize = 12;
/// Signals the picker shows at once (the trace answers at most this many).
pub const MAX_SHOWN: usize = 128;
/// Longest symbol shown whole in the trace strip; longer ones are elided.
pub const SYMBOL_MAX: usize = 48;

/// The picker's signal groups, in the order its buttons are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Board,
    Uart,
    Bus,
    Pmod,
    Other,
}

impl Group {
    /// Every group, in display order.
    pub const ALL: [Group; 5] = [
        Group::Board,
        Group::Uart,
        Group::Bus,
        Group::Pmod,
        Group::Other,
    ];

    /// The button and tooltip word.
    pub fn label(self) -> &'static str {
        match self {
            Group::Board => "board",
            Group::Uart => "uart",
            Group::Bus => "bus",
            Group::Pmod => "pmod",
            Group::Other => "other",
        }
    }
}

/// The group of a signal, from its trace name (`board.led0`, `uart1.tx`,
/// `bus.M_AXI_GP0.addr`, `pmod.ja`, `hdmi.tx.frame`...).
pub fn group_of(name: &str) -> Group {
    if name.starts_with("board.") {
        Group::Board
    } else if name.starts_with("uart") {
        Group::Uart
    } else if name.starts_with("bus.") {
        Group::Bus
    } else if name.starts_with("pmod.") {
        Group::Pmod
    } else {
        Group::Other
    }
}

/// The signals a project opens with: LEDs, switches, buttons, UART bytes,
/// Pmod pins, the RGB LEDs and one bus address, at most [`DEFAULT_SIGNALS`].
/// Each kind has a share first, so a board with many LEDs still shows its
/// UART; room left over is filled in the same order.
pub fn default_signals(catalogue: &[SignalInfo]) -> Vec<String> {
    type Rule = (usize, fn(&str) -> bool);
    let rules: [Rule; 7] = [
        (3, |n| n.starts_with("board.led")),
        (2, |n| n.starts_with("board.sw")),
        (1, |n| n.starts_with("board.btn")),
        (2, |n| n.starts_with("uart")),
        (2, |n| n.starts_with("pmod.")),
        (1, |n| n == "board.rgb_pins"),
        (1, |n| n.starts_with("bus.") && n.ends_with(".addr")),
    ];
    let mut out: Vec<String> = Vec::new();
    for shares in [true, false] {
        for (share, wanted) in rules {
            let mut taken = 0;
            for s in catalogue {
                if out.len() >= DEFAULT_SIGNALS {
                    return out;
                }
                if shares && taken >= share {
                    break;
                }
                if wanted(&s.name) && !out.contains(&s.name) {
                    out.push(s.name.clone());
                    taken += 1;
                }
            }
        }
    }
    out
}

/// Shortens `s` to at most `max` characters: its middle becomes "...", the
/// start and the end stay (they tell a mangled symbol apart). Text that fits
/// is returned unchanged.
pub fn elide_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let keep = max - 3;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let start: String = s.chars().take(head).collect();
    let end: String = s.chars().skip(n - tail).collect();
    format!("{start}...{end}")
}

/// `line` with each long symbol (a mangled `_ZN...` name or a `::` path
/// longer than [`SYMBOL_MAX`]) elided, or `None` when there is none.
pub fn elide_long_symbols(line: &str) -> Option<String> {
    let mut changed = false;
    let out: Vec<String> = line
        .split(' ')
        .map(|tok| {
            let symbol = tok.starts_with("_ZN") || tok.contains("::");
            if symbol && tok.chars().count() > SYMBOL_MAX {
                changed = true;
                elide_middle(tok, SYMBOL_MAX)
            } else {
                tok.to_string()
            }
        })
        .collect();
    changed.then(|| out.join(" "))
}

/// Everything the waveform panel shows and does, except drawing.
#[derive(Default)]
pub struct WaveState {
    /// Signals shown, in display order. Survives a project reload.
    pub selected: Vec<String>,
    /// Whether the defaults were chosen for this project (or not needed):
    /// they are chosen once, when the first signals appear.
    defaults_done: bool,
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

    /// Chooses the default signals once, when the first ones are known and
    /// nothing was picked yet (a new project).
    fn apply_defaults(&mut self) {
        if self.defaults_done || self.catalogue.is_empty() {
            return;
        }
        self.defaults_done = true;
        if self.selected.is_empty() {
            self.selected = default_signals(&self.catalogue);
        }
    }

    /// The "Pick default signals" button: replaces the selection with the
    /// defaults.
    pub fn pick_defaults(&mut self) {
        self.defaults_done = true;
        self.selected = default_signals(&self.catalogue);
    }

    /// Adds every signal the search text matches (up to [`MAX_SHOWN`]).
    pub fn select_matching(&mut self) {
        let names: Vec<String> = self.filtered().iter().map(|s| s.name.clone()).collect();
        self.add_names(names);
    }

    /// Adds every signal of `group` (up to [`MAX_SHOWN`]).
    pub fn select_group(&mut self, group: Group) {
        let names: Vec<String> = self
            .catalogue
            .iter()
            .filter(|s| group_of(&s.name) == group)
            .map(|s| s.name.clone())
            .collect();
        self.add_names(names);
    }

    /// Hides every signal.
    pub fn select_none(&mut self) {
        self.selected.clear();
    }

    /// How many catalogue signals are in `group`.
    pub fn group_count(&self, group: Group) -> usize {
        self.catalogue
            .iter()
            .filter(|s| group_of(&s.name) == group)
            .count()
    }

    fn add_names(&mut self, names: Vec<String>) {
        for n in names {
            if self.selected.len() >= MAX_SHOWN {
                break;
            }
            if !self.is_selected(&n) {
                self.selected.push(n);
            }
        }
    }

    /// Shows the whole run, from time 0 to the newest record.
    pub fn fit(&mut self) {
        self.from_ps = 0;
        self.span_ps = self.now_ps.max(MIN_SPAN_PS);
        self.follow = false;
    }

    /// Shows the time from `from` to `to` (picoseconds), at least
    /// [`MIN_SPAN_PS`] wide; leaves follow mode. Used by "zoom to selection".
    pub fn zoom_to(&mut self, from: u64, to: u64) {
        self.from_ps = from;
        self.span_ps = to.saturating_sub(from).max(MIN_SPAN_PS);
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
        self.waves.apply_defaults();
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

    fn info(n: &str) -> SignalInfo {
        SignalInfo {
            name: n.to_string(),
            width: 1,
            changes: 1,
            first_ps: 0,
            last_ps: 0,
            derived: false,
        }
    }

    #[test]
    fn default_signals_cover_each_kind_and_stay_within_the_cap() {
        let names = [
            "bus.M_AXI_GP0.addr",
            "uart1.tx",
            "uart1.rx",
            "uart0.tx",
            "board.led0",
            "board.led1",
            "board.led2",
            "board.led3",
            "board.led4",
            "board.sw0",
            "board.sw1",
            "board.sw2",
            "board.btn0",
            "board.btn1",
            "board.rgb_pins",
            "pmod.ja",
            "pmod.jb",
            "hdmi.tx.frame",
            "codec.r4",
        ];
        let cat: Vec<SignalInfo> = names.iter().map(|n| info(n)).collect();
        let d = default_signals(&cat);
        assert!(d.len() <= DEFAULT_SIGNALS, "{d:?}");
        // Every kind is represented, and LEDs come first.
        assert_eq!(d[0], "board.led0");
        for want in [
            "board.sw0",
            "board.btn0",
            "uart1.tx",
            "uart1.rx",
            "pmod.ja",
            "board.rgb_pins",
            "bus.M_AXI_GP0.addr",
        ] {
            assert!(d.iter().any(|n| n == want), "missing {want}: {d:?}");
        }
        // Only the useful kinds are chosen while there is room for them.
        assert!(
            !d.iter()
                .any(|n| n.starts_with("hdmi") || n.starts_with("codec"))
        );
        // A catalogue with many LEDs does not crowd out the UART.
        let many: Vec<SignalInfo> = (0..40)
            .map(|i| info(&format!("board.led{i}")))
            .chain([info("uart1.tx")])
            .collect();
        let d = default_signals(&many);
        assert_eq!(d.len(), DEFAULT_SIGNALS);
        assert!(d.iter().any(|n| n == "uart1.tx"), "{d:?}");
    }

    #[test]
    fn groups_and_the_picker_actions() {
        assert_eq!(group_of("board.sw0"), Group::Board);
        assert_eq!(group_of("uart1.rx"), Group::Uart);
        assert_eq!(group_of("bus.M_AXI_GP0.write"), Group::Bus);
        assert_eq!(group_of("pmod.ja"), Group::Pmod);
        assert_eq!(group_of("hdmi.tx.frame"), Group::Other);

        let mut w = WaveState {
            catalogue: vec![
                info("board.led0"),
                info("uart1.tx"),
                info("uart1.rx"),
                info("pmod.ja"),
            ],
            ..WaveState::default()
        };
        assert_eq!(w.group_count(Group::Uart), 2);
        assert_eq!(w.group_count(Group::Bus), 0);
        w.select_group(Group::Uart);
        assert_eq!(w.selected, ["uart1.tx", "uart1.rx"]);
        // Groups add to the selection without duplicates.
        w.select_group(Group::Uart);
        w.select_group(Group::Board);
        assert_eq!(w.selected, ["uart1.tx", "uart1.rx", "board.led0"]);
        w.select_none();
        assert!(w.selected.is_empty());
        w.filter = "pmod".to_string();
        w.select_matching();
        assert_eq!(w.selected, ["pmod.ja"]);
        // The first pick replaces an empty selection with the defaults.
        w.select_none();
        w.pick_defaults();
        assert!(w.selected.contains(&"board.led0".to_string()));
        // Once picked (even emptied), the defaults are not forced again.
        w.select_none();
        w.apply_defaults();
        assert!(w.selected.is_empty());
    }

    #[test]
    fn a_new_project_opens_with_the_default_signals() {
        let mut m = model();
        run(&mut m, 1_000_000_000);
        m.waves.active = true;
        m.refresh();
        let sel = &m.waves.selected;
        assert!(!sel.is_empty() && sel.len() <= DEFAULT_SIGNALS, "{sel:?}");
        assert!(sel.iter().any(|n| n == "board.led0"), "{sel:?}");
        assert!(sel.iter().any(|n| n.starts_with("uart")), "{sel:?}");
    }

    #[test]
    fn long_symbols_are_elided_in_the_middle_and_short_text_is_kept() {
        assert_eq!(elide_middle("short", 10), "short");
        let s = "_ZN9firmware4main17h0123456789abcdefE_very_long_tail_end";
        let e = elide_middle(s, 20);
        assert_eq!(e.chars().count(), 20);
        assert!(e.starts_with("_ZN9firm") && e.ends_with("tail_end"), "{e}");
        assert!(e.contains("..."));
        assert_eq!(elide_middle("abcdef", 2), "ab");
        // Multi-byte text is cut on character boundaries.
        assert_eq!(elide_middle("ööööööööööööö", 6).chars().count(), 6);

        let line = format!("pc in {s} at 0x100");
        let out = elide_long_symbols(&line).expect("the symbol is long");
        assert!(out.starts_with("pc in _ZN") && out.ends_with(" at 0x100"));
        assert!(out.len() < line.len());
        assert_eq!(elide_long_symbols("uart1 tx A"), None);
        let path = "core::fmt::Formatter::pad_integral::with_long_name_for_tests::xyz";
        assert!(elide_long_symbols(path).unwrap().contains("..."));
    }

    #[test]
    fn zoom_to_selection_sets_the_window_and_leaves_follow() {
        let mut w = WaveState {
            now_ps: 1_000_000,
            follow: true,
            ..WaveState::default()
        };
        w.zoom_to(200_000, 300_000);
        assert_eq!((w.from_ps, w.span_ps), (200_000, 100_000));
        assert!(!w.follow);
        // A selection narrower than the minimum still shows MIN_SPAN_PS.
        w.zoom_to(5, 5);
        assert_eq!((w.from_ps, w.span_ps), (5, MIN_SPAN_PS));
    }
}
