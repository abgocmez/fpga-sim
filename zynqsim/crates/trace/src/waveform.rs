//! Waveform queries over a [`TraceLog`]: the catalogue of signals the trace
//! holds and the value changes of chosen signals inside a time window.
//!
//! The signals are the same ones the VCD export writes (named `Signal`
//! records and UART bytes), plus three signals *derived* from every PS/PL bus
//! access, `bus.<port>.addr`, `bus.<port>.data` and `bus.<port>.write`, which
//! the VCD export does not contain. Nothing here reads the PL's internal
//! nets: only what the engine recorded at the boundary is available.
//!
//! Every answer is bounded: a window holds at most `max_changes` value
//! changes per signal and at most [`MAX_SIGNALS`] signals, and says so
//! explicitly when it had to cut.

use crate::log::{TraceEvent, TraceLog};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Value changes returned per signal when the caller does not say.
pub const DEFAULT_MAX_CHANGES: usize = 2_000;
/// Upper bound on `max_changes`.
pub const HARD_MAX_CHANGES: usize = 20_000;
/// Most signals one window answers for (wildcards are cut here).
pub const MAX_SIGNALS: usize = 128;

/// One signal of the catalogue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalInfo {
    /// Hierarchical name, for example `board.led0` or `uart1.tx`.
    pub name: String,
    /// Width in bits (the widest value seen), 1..=64.
    pub width: u8,
    /// Value changes (samples) in the retained trace.
    pub changes: usize,
    /// Time of the first retained sample, in picoseconds.
    pub first_ps: u64,
    /// Time of the last retained sample, in picoseconds.
    pub last_ps: u64,
    /// True for a signal synthesised from bus accesses; it is not in the VCD.
    pub derived: bool,
}

/// The value of one signal changing at one time.
pub type Change = (u64, u64);

/// One signal's part of a window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalWave {
    /// The signal name as requested (wildcards are expanded to real names).
    pub name: String,
    /// Width in bits; 0 when the signal is not in the trace.
    pub width: u8,
    /// False when no retained record names this signal.
    pub found: bool,
    /// The value in effect at `from_ps` (the last sample before it), if the
    /// signal had been driven by then.
    pub initial: Option<u64>,
    /// `(time_ps, value)` samples inside the window, oldest first. Samples
    /// at the same time keep their trace order.
    pub changes: Vec<Change>,
    /// Samples the window holds for this signal, before limiting.
    pub in_window: usize,
    /// True when `changes` is only the first part of the window. Nothing is
    /// known to the viewer after `covered_to_ps`.
    pub truncated: bool,
    /// Time up to which `changes` is complete (`to_ps` unless truncated).
    pub covered_to_ps: u64,
}

impl SignalWave {
    /// The value at time `t` (the last sample at or before `t`), if driven.
    /// Valid for `from_ps <= t <= covered_to_ps` of the window.
    pub fn value_at(&self, t: u64) -> Option<u64> {
        let after = self.changes.partition_point(|c| c.0 <= t);
        if after == 0 {
            self.initial
        } else {
            Some(self.changes[after - 1].1)
        }
    }
}

/// The answer to a waveform query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waveform {
    /// Start of the window, in picoseconds.
    pub from_ps: u64,
    /// End of the window (inclusive), in picoseconds.
    pub to_ps: u64,
    /// Time of the newest record in the trace, in picoseconds.
    pub now_ps: u64,
    /// Records ever recorded (including dropped ones).
    pub trace_len: usize,
    /// Records dropped from the front of a bounded trace; values before the
    /// first retained record are unknown.
    pub dropped: usize,
    /// The per-signal results, in request order (wildcards in name order).
    pub signals: Vec<SignalWave>,
    /// True when more than [`MAX_SIGNALS`] signals matched and some were cut.
    pub signals_truncated: bool,
    /// The per-signal limit that was applied.
    pub max_changes: usize,
}

impl Waveform {
    /// True when any signal's list is cut short.
    pub fn truncated(&self) -> bool {
        self.signals_truncated || self.signals.iter().any(|s| s.truncated)
    }
}

/// The derived bus signal names for one port.
struct BusNames {
    addr: String,
    data: String,
    write: String,
}

impl BusNames {
    fn new(port: &str) -> BusNames {
        BusNames {
            addr: format!("bus.{port}.addr"),
            data: format!("bus.{port}.data"),
            write: format!("bus.{port}.write"),
        }
    }
}

fn uart_name(port: u8, tx: bool) -> String {
    format!("uart{port}.{}", if tx { "tx" } else { "rx" })
}

/// Calls `f(name, value, width, derived)` for every waveform sample a record
/// contributes. `bus` caches the derived names per port.
fn samples<'a>(
    ev: &'a TraceEvent,
    bus: &mut HashMap<&'a str, BusNames>,
    f: &mut dyn FnMut(&str, u64, u8, bool),
) {
    match ev {
        TraceEvent::Signal { name, value, width } => f(name, *value, *width, false),
        TraceEvent::Uart { port, tx, byte } => {
            f(&uart_name(*port, *tx), u64::from(*byte), 8, false);
        }
        TraceEvent::Bus {
            port,
            write,
            addr,
            len,
            data,
            ..
        } => {
            let n = bus
                .entry(port.as_str())
                .or_insert_with(|| BusNames::new(port));
            f(&n.addr, *addr, 32, true);
            f(&n.data, *data, if *len > 4 { 64 } else { 32 }, true);
            f(&n.write, u64::from(*write), 1, true);
        }
        TraceEvent::Input { .. } | TraceEvent::Marker { .. } => {}
    }
}

/// Every signal in the retained trace, in name order.
pub fn signals(log: &TraceLog) -> Vec<SignalInfo> {
    let mut map: BTreeMap<String, SignalInfo> = BTreeMap::new();
    let mut bus = HashMap::new();
    for r in log.records() {
        let t = r.t.0;
        samples(&r.event, &mut bus, &mut |name, _, width, derived| {
            if let Some(s) = map.get_mut(name) {
                s.width = s.width.max(width);
                s.changes += 1;
                s.first_ps = s.first_ps.min(t);
                s.last_ps = s.last_ps.max(t);
            } else {
                map.insert(
                    name.to_string(),
                    SignalInfo {
                        name: name.to_string(),
                        width,
                        changes: 1,
                        first_ps: t,
                        last_ps: t,
                        derived,
                    },
                );
            }
        });
    }
    map.into_values().collect()
}

/// Time of the newest record (0 for an empty trace).
pub fn now_ps(log: &TraceLog) -> u64 {
    log.records().iter().map(|r| r.t.0).max().unwrap_or(0)
}

#[derive(Default)]
struct Acc {
    width: u8,
    found: bool,
    initial: Option<(u64, u64)>,
    window: Vec<Change>,
}

/// The value changes of `names` inside `[from_ps, to_ps]`.
///
/// A name ending in `*` stands for every catalogue signal with that prefix.
/// Unknown names are reported with `found = false`. `max_changes` is
/// clamped to `1..=`[`HARD_MAX_CHANGES`]. A window with `to_ps < from_ps` is
/// empty.
pub fn waveform(
    log: &TraceLog,
    names: &[String],
    from_ps: u64,
    to_ps: u64,
    max_changes: usize,
) -> Waveform {
    let max_changes = max_changes.clamp(1, HARD_MAX_CHANGES);

    // Expand wildcards against the catalogue (only scanned when needed).
    let mut wanted: Vec<String> = Vec::new();
    let mut signals_truncated = false;
    let catalogue = names.iter().any(|n| n.ends_with('*')).then(|| signals(log));
    for n in names {
        let expanded: Vec<String> = match (n.strip_suffix('*'), &catalogue) {
            (Some(prefix), Some(cat)) => cat
                .iter()
                .filter(|s| s.name.starts_with(prefix))
                .map(|s| s.name.clone())
                .collect(),
            _ => vec![n.clone()],
        };
        for e in expanded {
            if wanted.contains(&e) {
                continue;
            }
            if wanted.len() >= MAX_SIGNALS {
                signals_truncated = true;
            } else {
                wanted.push(e);
            }
        }
    }

    let index: HashMap<&str, usize> = wanted
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut acc: Vec<Acc> = wanted.iter().map(|_| Acc::default()).collect();
    let mut bus = HashMap::new();
    let mut now = 0u64;
    // Only synthesise names for records that could be wanted.
    let wants_bus = wanted.iter().any(|n| n.starts_with("bus."));
    let wants_uart = wanted.iter().any(|n| n.starts_with("uart"));
    for r in log.records() {
        let t = r.t.0;
        now = now.max(t);
        match &r.event {
            TraceEvent::Bus { .. } if !wants_bus => continue,
            TraceEvent::Uart { .. } if !wants_uart => continue,
            _ => {}
        }
        samples(&r.event, &mut bus, &mut |name, value, width, _| {
            let Some(&i) = index.get(name) else { return };
            let a = &mut acc[i];
            a.found = true;
            a.width = a.width.max(width);
            if t < from_ps {
                if a.initial.is_none_or(|(it, _)| t >= it) {
                    a.initial = Some((t, value));
                }
            } else if t <= to_ps {
                a.window.push((t, value));
            }
        });
    }

    let signals = wanted
        .into_iter()
        .zip(acc)
        .map(|(name, mut a)| {
            // Stable: samples at one time keep their trace order.
            a.window.sort_by_key(|c| c.0);
            let in_window = a.window.len();
            let truncated = in_window > max_changes;
            if truncated {
                a.window.truncate(max_changes);
            }
            let covered_to_ps = if truncated {
                a.window.last().map_or(from_ps, |c| c.0)
            } else {
                to_ps
            };
            SignalWave {
                name,
                width: a.width,
                found: a.found,
                initial: a.initial.map(|(_, v)| v),
                changes: a.window,
                in_window,
                truncated,
                covered_to_ps,
            }
        })
        .collect();

    Waveform {
        from_ps,
        to_ps,
        now_ps: now,
        trace_len: log.len(),
        dropped: log.dropped(),
        signals,
        signals_truncated,
        max_changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Ps;

    fn sig(l: &mut TraceLog, t: u64, name: &str, value: u64, width: u8) {
        l.push(
            Ps(t),
            TraceEvent::Signal {
                name: name.to_string(),
                value,
                width,
            },
        );
    }

    fn log() -> TraceLog {
        let mut l = TraceLog::new();
        for i in 0..10u64 {
            sig(&mut l, i * 100, "a", i & 1, 1);
        }
        sig(&mut l, 250, "bus8", 0xAB, 8);
        sig(&mut l, 750, "bus8", 0xCD, 8);
        l.push(Ps(300), TraceEvent::Marker { text: "m".into() });
        l.push(
            Ps(400),
            TraceEvent::Uart {
                port: 1,
                tx: true,
                byte: b'H',
            },
        );
        l.push(
            Ps(500),
            TraceEvent::Bus {
                port: "M_AXI_GP0".into(),
                write: true,
                addr: 0x43C0_0000,
                len: 4,
                data: 7,
                ok: true,
            },
        );
        l
    }

    #[test]
    fn the_catalogue_lists_signals_uarts_and_derived_bus_signals() {
        let c = signals(&log());
        let names: Vec<&str> = c.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "a",
                "bus.M_AXI_GP0.addr",
                "bus.M_AXI_GP0.data",
                "bus.M_AXI_GP0.write",
                "bus8",
                "uart1.tx"
            ]
        );
        let a = &c[0];
        assert_eq!((a.width, a.changes, a.first_ps, a.last_ps), (1, 10, 0, 900));
        assert!(!a.derived && c[1].derived);
        assert_eq!(c[1].width, 32);
        assert_eq!(c[5].width, 8);
        assert_eq!(now_ps(&log()), 900);
    }

    #[test]
    fn a_window_holds_the_initial_value_and_the_samples_inside() {
        let w = waveform(&log(), &["a".into()], 250, 650, 100);
        let a = &w.signals[0];
        // Samples at 0..900 step 100 alternate 0,1,0,1...: the last before
        // 250 is the one at 200 (value 0).
        assert_eq!(a.initial, Some(0));
        let t: Vec<u64> = a.changes.iter().map(|c| c.0).collect();
        assert_eq!(t, [300, 400, 500, 600]);
        assert_eq!(a.changes[0], (300, 1));
        assert!(!a.truncated && !w.truncated());
        assert_eq!(a.covered_to_ps, 650);
        // Boundaries are inclusive at both ends.
        let w = waveform(&log(), &["a".into()], 300, 600, 100);
        assert_eq!(w.signals[0].changes.len(), 4);
        assert_eq!(w.signals[0].initial, Some(0));
        // A window before the first sample has no initial value.
        let w = waveform(&log(), &["bus8".into()], 0, 100, 10);
        assert_eq!(w.signals[0].initial, None);
        assert!(w.signals[0].changes.is_empty());
        assert!(w.signals[0].found);
        assert_eq!(w.signals[0].width, 8);
    }

    #[test]
    fn value_at_uses_the_initial_value_and_the_last_change() {
        let w = waveform(&log(), &["bus8".into()], 500, 1000, 10);
        let s = &w.signals[0];
        assert_eq!(s.initial, Some(0xAB));
        assert_eq!(s.value_at(500), Some(0xAB));
        assert_eq!(s.value_at(749), Some(0xAB));
        assert_eq!(s.value_at(750), Some(0xCD));
        assert_eq!(s.value_at(999), Some(0xCD));
    }

    #[test]
    fn a_long_window_is_truncated_and_says_where() {
        let w = waveform(&log(), &["a".into()], 0, 1000, 3);
        let a = &w.signals[0];
        assert!(a.truncated && w.truncated());
        assert_eq!(a.in_window, 10);
        assert_eq!(a.changes.len(), 3);
        assert_eq!(a.covered_to_ps, 200, "complete up to the last kept sample");
        // The limit is clamped, never zero.
        let w = waveform(&log(), &["a".into()], 0, 1000, 0);
        assert_eq!((w.max_changes, w.signals[0].changes.len()), (1, 1));
        let w = waveform(&log(), &["a".into()], 0, 1000, usize::MAX);
        assert_eq!(w.max_changes, HARD_MAX_CHANGES);
    }

    #[test]
    fn requested_order_is_kept_wildcards_expand_and_unknowns_are_flagged() {
        let names = ["uart1.tx", "nope", "bus.*", "a"].map(String::from);
        let w = waveform(&log(), &names, 0, 1000, 10);
        let got: Vec<(&str, bool)> = w
            .signals
            .iter()
            .map(|s| (s.name.as_str(), s.found))
            .collect();
        assert_eq!(
            got,
            [
                ("uart1.tx", true),
                ("nope", false),
                ("bus.M_AXI_GP0.addr", true),
                ("bus.M_AXI_GP0.data", true),
                ("bus.M_AXI_GP0.write", true),
                ("a", true),
            ]
        );
        assert_eq!(w.signals[0].changes, [(400, u64::from(b'H'))]);
        assert_eq!(w.signals[2].changes, [(500, 0x43C0_0000)]);
        assert_eq!(w.signals[4].changes, [(500, 1)]);
        // A duplicate request is answered once.
        let names = ["a", "a"].map(String::from);
        assert_eq!(waveform(&log(), &names, 0, 10, 10).signals.len(), 1);
    }

    #[test]
    fn samples_are_ordered_by_time_even_when_records_arrive_late() {
        let mut l = TraceLog::new();
        sig(&mut l, 500, "s", 1, 1);
        sig(&mut l, 100, "s", 0, 1);
        sig(&mut l, 100, "s", 1, 1);
        sig(&mut l, 50, "s", 1, 1);
        let w = waveform(&l, &["s".into()], 60, 1000, 10);
        let s = &w.signals[0];
        assert_eq!(s.initial, Some(1));
        assert_eq!(s.changes, [(100, 0), (100, 1), (500, 1)], "stable ties");
    }

    #[test]
    fn an_empty_or_inverted_window_is_empty() {
        let w = waveform(&log(), &["a".into()], 700, 100, 10);
        assert!(w.signals[0].changes.is_empty());
        let w = waveform(&TraceLog::new(), &["a".into()], 0, 100, 10);
        assert!(!w.signals[0].found);
        assert_eq!(w.now_ps, 0);
    }

    #[test]
    fn too_many_signals_are_cut_with_a_flag() {
        let mut l = TraceLog::new();
        for i in 0..(MAX_SIGNALS + 5) {
            sig(&mut l, 0, &format!("s{i:03}"), 1, 1);
        }
        let w = waveform(&l, &["s*".into()], 0, 10, 10);
        assert_eq!(w.signals.len(), MAX_SIGNALS);
        assert!(w.signals_truncated && w.truncated());
    }

    #[test]
    fn a_bounded_log_reports_its_dropped_records() {
        let mut l = TraceLog::with_limit(10);
        for i in 0..100u64 {
            sig(&mut l, i, "s", i & 1, 1);
        }
        let w = waveform(&l, &["s".into()], 0, 1000, 1000);
        assert!(w.dropped > 0);
        assert_eq!(w.trace_len, 100);
        assert_eq!(w.now_ps, 99);
    }
}
