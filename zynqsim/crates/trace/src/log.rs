use crate::hash::Fnv64;
use serde::{Deserialize, Serialize};
use sim_core::Ps;

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceEvent {
    /// A front-end input applied to the simulation, described as text.
    Input {
        /// Human-readable description, for example `switch 2 = on`.
        what: String,
    },
    /// A named signal changed value.
    Signal {
        /// Hierarchical name, for example `board.led0`.
        name: String,
        /// New value (low `width` bits are significant).
        value: u64,
        /// Width in bits, 1..=64.
        width: u8,
    },
    /// One byte crossed a UART.
    Uart {
        /// UART index (0 or 1).
        port: u8,
        /// True for target-to-host (TX), false for host-to-target (RX).
        tx: bool,
        /// The byte.
        byte: u8,
    },
    /// A free-form marker (reset, run boundaries, diagnostics).
    Marker {
        /// Marker text.
        text: String,
    },
    /// One bus access between the PS and the PL.
    Bus {
        /// Port name, for example `M_AXI_GP0`.
        port: String,
        /// True for a write, false for a read.
        write: bool,
        /// Address.
        addr: u64,
        /// Access length in bytes.
        len: u32,
        /// The first up to 8 data bytes, little-endian.
        data: u64,
        /// False if the access completed with an error response.
        ok: bool,
    },
}

impl TraceEvent {
    /// Feeds the canonical byte encoding of this event into `h`.
    ///
    /// The encoding is independent of serde so that changing a JSON field name
    /// cannot silently change every recorded hash.
    fn feed(&self, h: &mut Fnv64) {
        match self {
            TraceEvent::Input { what } => {
                h.write(&[1]);
                h.write_str(what);
            }
            TraceEvent::Signal { name, value, width } => {
                h.write(&[2]);
                h.write_str(name);
                h.write_u64(*value);
                h.write(&[*width]);
            }
            TraceEvent::Uart { port, tx, byte } => {
                h.write(&[3, *port, u8::from(*tx), *byte]);
            }
            TraceEvent::Marker { text } => {
                h.write(&[4]);
                h.write_str(text);
            }
            TraceEvent::Bus {
                port,
                write,
                addr,
                len,
                data,
                ok,
            } => {
                h.write(&[5]);
                h.write_str(port);
                h.write(&[u8::from(*write), u8::from(*ok)]);
                h.write_u64(*addr);
                h.write_u64(u64::from(*len));
                h.write_u64(*data);
            }
        }
    }
}

/// A timestamped event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    /// Simulation time of the event.
    pub t: Ps,
    /// The event.
    #[serde(flatten)]
    pub event: TraceEvent,
}

/// An append-only event log with an incrementally maintained digest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<TraceRecord>", into = "Vec<TraceRecord>")]
pub struct TraceLog {
    records: Vec<TraceRecord>,
    hash: Fnv64,
    /// Records discarded from the front to honour `limit`.
    dropped: usize,
    /// Most records to keep, if bounded. The digest always covers every record.
    limit: Option<usize>,
}

impl From<Vec<TraceRecord>> for TraceLog {
    fn from(records: Vec<TraceRecord>) -> TraceLog {
        let mut log = TraceLog::default();
        for r in records {
            log.push(r.t, r.event);
        }
        log
    }
}

impl From<TraceLog> for Vec<TraceRecord> {
    fn from(log: TraceLog) -> Vec<TraceRecord> {
        log.records
    }
}

impl TraceLog {
    /// An empty log that keeps every record.
    pub fn new() -> TraceLog {
        TraceLog::default()
    }

    /// An empty log that keeps only the newest `limit` records (a long run of
    /// a polling program would otherwise grow without bound). The digest and
    /// [`TraceLog::len`] still count every record; old ones are removed in
    /// batches, so a few more than `limit` may be retained at times.
    pub fn with_limit(limit: usize) -> TraceLog {
        TraceLog {
            limit: Some(limit.max(1)),
            ..TraceLog::default()
        }
    }

    /// Appends an event at time `t`.
    pub fn push(&mut self, t: Ps, event: TraceEvent) {
        self.hash.write_u64(t.0);
        event.feed(&mut self.hash);
        self.records.push(TraceRecord { t, event });
        if let Some(l) = self.limit {
            if self.records.len() > l + l / 8 + 1 {
                let n = self.records.len() - l;
                self.records.drain(..n);
                self.dropped += n;
            }
        }
    }

    /// The retained records in order; the first one has absolute index
    /// [`TraceLog::first_index`].
    pub fn records(&self) -> &[TraceRecord] {
        &self.records
    }

    /// Absolute index of the first retained record (0 unless bounded and full).
    pub fn first_index(&self) -> usize {
        self.dropped
    }

    /// Records discarded to stay within the limit.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Up to `max` records starting at absolute index `from`, and the absolute
    /// index of the first one returned (later than `from` if it was dropped).
    pub fn window(&self, from: usize, max: usize) -> (usize, &[TraceRecord]) {
        let start = from.max(self.dropped).min(self.len());
        let lo = start - self.dropped;
        let hi = lo.saturating_add(max).min(self.records.len());
        (start, &self.records[lo..hi])
    }

    /// Number of records ever recorded (including dropped ones).
    pub fn len(&self) -> usize {
        self.dropped + self.records.len()
    }

    /// True when nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The digest as `fnv1a64:` followed by 16 lower-case hex digits.
    pub fn hash_string(&self) -> String {
        format!("fnv1a64:{:016x}", self.hash.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bounded_log_keeps_the_digest_and_absolute_indices() {
        let mut full = TraceLog::new();
        let mut small = TraceLog::with_limit(100);
        for i in 0..1000u64 {
            let e = TraceEvent::Marker {
                text: format!("m{i}"),
            };
            full.push(Ps(i), e.clone());
            small.push(Ps(i), e);
        }
        assert_eq!(small.hash_string(), full.hash_string(), "digest covers all");
        assert_eq!(small.len(), 1000, "len counts dropped records");
        assert!(small.records().len() <= 100 + 100 / 8 + 1);
        assert_eq!(small.first_index() + small.records().len(), 1000);
        assert_eq!(small.dropped(), small.first_index());
        // The newest records are the same as the unbounded log's.
        assert_eq!(
            small.records().last(),
            full.records().last(),
            "newest retained"
        );
        // A window by absolute index: dropped part is skipped, not shifted.
        let (start, recs) = small.window(0, 5);
        assert_eq!(start, small.first_index());
        assert_eq!(recs[0], full.records()[start]);
        let (start, recs) = small.window(995, 50);
        assert_eq!((start, recs.len()), (995, 5));
        assert_eq!(recs[0], full.records()[995]);
        let (start, recs) = small.window(5000, 10);
        assert_eq!((start, recs.len()), (1000, 0));
        assert!(!small.is_empty());
    }

    fn sample() -> TraceLog {
        let mut l = TraceLog::new();
        l.push(
            Ps(0),
            TraceEvent::Marker {
                text: "reset".into(),
            },
        );
        l.push(
            Ps(10),
            TraceEvent::Signal {
                name: "board.led0".into(),
                value: 1,
                width: 1,
            },
        );
        l.push(
            Ps(20),
            TraceEvent::Uart {
                port: 1,
                tx: true,
                byte: b'A',
            },
        );
        l
    }

    #[test]
    fn hash_depends_on_time_and_content() {
        let a = sample();
        let b = sample();
        assert_eq!(a.hash_string(), b.hash_string());

        let mut c = TraceLog::new();
        for r in a.records() {
            let t = if r.t == Ps(10) { Ps(11) } else { r.t };
            c.push(t, r.event.clone());
        }
        assert_ne!(a.hash_string(), c.hash_string());
    }

    #[test]
    fn serde_round_trip_restores_hash() {
        let a = sample();
        let json = serde_json::to_string(&a).unwrap();
        let b: TraceLog = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.hash_string(), b.hash_string());
    }

    #[test]
    fn bus_events_hash_and_round_trip() {
        let mk = |ok| {
            let mut l = TraceLog::new();
            l.push(
                Ps(5),
                TraceEvent::Bus {
                    port: "M_AXI_GP0".into(),
                    write: true,
                    addr: 0x43C0_0000,
                    len: 4,
                    data: 0x15,
                    ok,
                },
            );
            l
        };
        assert_ne!(mk(true).hash_string(), mk(false).hash_string());
        let json = serde_json::to_string(&mk(true)).unwrap();
        let back: TraceLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mk(true));
        assert!(json.contains("\"type\":\"bus\""));
    }

    #[test]
    fn json_shape_is_flat_and_tagged() {
        let json = serde_json::to_value(sample().records()[1].clone()).unwrap();
        assert_eq!(json["t"], 10);
        assert_eq!(json["type"], "signal");
        assert_eq!(json["name"], "board.led0");
    }
}
