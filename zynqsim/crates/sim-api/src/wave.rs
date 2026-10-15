//! `Query::SignalCatalogue` and `Query::Waveform`: both engines answer them
//! from their trace log with the same code (ADR-014).

use crate::error::ApiError;
use crate::types::{Query, Response};
use trace::TraceLog;
use trace::waveform::{DEFAULT_MAX_CHANGES, now_ps, signals, waveform};

/// Answers a waveform-related query from `log`.
pub(crate) fn answer(log: &TraceLog, q: &Query) -> Result<Response, ApiError> {
    match q {
        Query::SignalCatalogue => Ok(Response::Signals {
            signals: signals(log),
            now_ps: now_ps(log),
            trace_len: log.len(),
        }),
        Query::Waveform {
            signals,
            from_ps,
            to_ps,
            max_changes,
        } => {
            if signals.is_empty() {
                return Err(ApiError::invalid_argument(
                    "a waveform query needs at least one signal name",
                )
                .with_hint(
                    "list the names with the signal catalogue; `prefix*` matches by prefix",
                ));
            }
            if to_ps < from_ps {
                return Err(ApiError::invalid_argument(format!(
                    "the window ends ({to_ps} ps) before it starts ({from_ps} ps)"
                )));
            }
            let max = if *max_changes == 0 {
                DEFAULT_MAX_CHANGES
            } else {
                *max_changes
            };
            Ok(Response::Waveform(Box::new(waveform(
                log, signals, *from_ps, *to_ps, max,
            ))))
        }
        other => Err(ApiError::internal(format!(
            "not a waveform query: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Input, SimConfig, Simulator};

    fn demo() -> Simulator {
        let mut s = Simulator::new(SimConfig::default()).unwrap();
        s.command(Command::Input(Input::SetSwitch { index: 1, on: true }))
            .unwrap();
        s.command(Command::Run {
            duration_ps: 2_000_000_000_000,
        })
        .unwrap();
        s
    }

    fn wave(
        s: &Simulator,
        names: &[&str],
        from: u64,
        to: u64,
        max: usize,
    ) -> trace::waveform::Waveform {
        match s
            .query(
                &Query::Waveform {
                    signals: names.iter().map(|n| n.to_string()).collect(),
                    from_ps: from,
                    to_ps: to,
                    max_changes: max,
                },
                false,
            )
            .unwrap()
        {
            Response::Waveform(w) => *w,
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn the_demo_catalogue_lists_board_and_uart_signals() {
        let s = demo();
        let Response::Signals {
            signals,
            now_ps,
            trace_len,
        } = s.query(&Query::SignalCatalogue, false).unwrap()
        else {
            panic!("not a catalogue");
        };
        let names: Vec<&str> = signals.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"board.sw1"), "{names:?}");
        assert!(names.contains(&"board.led3"), "{names:?}");
        assert!(names.contains(&"uart1.tx"), "{names:?}");
        assert!(names.windows(2).all(|w| w[0] < w[1]), "name order");
        assert_eq!(trace_len, s.trace().len());
        assert!(now_ps > 0);
        let sw = signals.iter().find(|s| s.name == "board.sw1").unwrap();
        assert_eq!(sw.width, 1);
        assert!(!sw.derived);
    }

    #[test]
    fn windows_cut_and_truncation_match_the_whole_trace() {
        let s = demo();
        let led3 = wave(&s, &["board.led3"], 0, u64::MAX / 2, 10_000);
        let all = &led3.signals[0];
        assert!(all.found && all.changes.len() >= 3, "heartbeat toggles");
        assert!(!all.truncated);
        // Every change of the whole run is in the whole window, in order.
        assert!(all.changes.windows(2).all(|w| w[0].0 <= w[1].0));
        // A window around the second change holds exactly the changes in it.
        let t1 = all.changes[1].0;
        let w = wave(&s, &["board.led3"], t1, t1, 10);
        let part = &w.signals[0];
        assert_eq!(part.changes, [all.changes[1]]);
        assert_eq!(part.initial, Some(all.changes[0].1));
        assert_eq!(part.value_at(t1), Some(all.changes[1].1));
        // A tiny limit truncates and says where.
        let w = wave(&s, &["board.led3"], 0, u64::MAX / 2, 2);
        let cut = &w.signals[0];
        assert!(cut.truncated && w.truncated());
        assert_eq!(cut.changes, all.changes[..2]);
        assert_eq!(cut.covered_to_ps, all.changes[1].0);
        assert_eq!(cut.in_window, all.changes.len());
    }

    #[test]
    fn bad_requests_are_refused_with_a_reason() {
        let s = demo();
        let e = s
            .query(
                &Query::Waveform {
                    signals: vec![],
                    from_ps: 0,
                    to_ps: 1,
                    max_changes: 0,
                },
                false,
            )
            .unwrap_err();
        assert_eq!(e.code, crate::error::ErrorCode::InvalidArgument);
        let e = s
            .query(
                &Query::Waveform {
                    signals: vec!["board.led3".to_string()],
                    from_ps: 10,
                    to_ps: 1,
                    max_changes: 0,
                },
                false,
            )
            .unwrap_err();
        assert!(e.message.contains("before it starts"), "{e}");
        // A zero limit means the default, not an empty answer.
        let w = wave(&s, &["board.sw1"], 0, u64::MAX / 2, 0);
        assert_eq!(w.max_changes, DEFAULT_MAX_CHANGES);
        assert!(!w.signals[0].changes.is_empty());
    }
}
