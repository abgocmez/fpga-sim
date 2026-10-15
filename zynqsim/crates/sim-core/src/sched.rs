use crate::time::Ps;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies a scheduled event; also the final tie-break for ordering.
pub type EventId = u64;

/// Scheduling errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedError {
    /// The requested time is before the scheduler's current time.
    InPast {
        /// Requested time.
        requested: Ps,
        /// Current time.
        now: Ps,
    },
}

impl std::fmt::Display for SchedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedError::InPast { requested, now } => {
                write!(f, "cannot schedule at {requested}: current time is {now}")
            }
        }
    }
}

impl std::error::Error for SchedError {}

/// One serialisable queue entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry<E> {
    at: Ps,
    priority: u8,
    id: EventId,
    event: E,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State<E> {
    now: Ps,
    next_id: EventId,
    entries: Vec<Entry<E>>,
}

/// A deterministic discrete-event queue.
///
/// Events fire ordered by `(time, priority, creation id)`; a lower priority
/// number fires first. The creation id is a monotonically increasing counter,
/// so two events at the same time and priority always fire in the order they
/// were scheduled, independent of any hashing or allocation behaviour.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    from = "State<E>",
    into = "State<E>",
    bound(
        serialize = "E: Clone + Serialize",
        deserialize = "E: Deserialize<'de>"
    )
)]
pub struct Scheduler<E> {
    now: Ps,
    next_id: EventId,
    queue: BTreeMap<(Ps, u8, EventId), E>,
}

impl<E> Default for Scheduler<E> {
    fn default() -> Self {
        Scheduler {
            now: Ps::ZERO,
            next_id: 0,
            queue: BTreeMap::new(),
        }
    }
}

impl<E> Scheduler<E> {
    /// An empty scheduler at time zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current simulation time.
    pub fn now(&self) -> Ps {
        self.now
    }

    /// Number of pending events.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True when no events are pending.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The time of the earliest pending event.
    pub fn peek_time(&self) -> Option<Ps> {
        self.queue.keys().next().map(|k| k.0)
    }

    /// Schedules `event` at absolute time `at`.
    pub fn schedule_at(&mut self, at: Ps, priority: u8, event: E) -> Result<EventId, SchedError> {
        if at < self.now {
            return Err(SchedError::InPast {
                requested: at,
                now: self.now,
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.queue.insert((at, priority, id), event);
        Ok(id)
    }

    /// Schedules `event` `delta` after the current time.
    pub fn schedule_in(&mut self, delta: Ps, priority: u8, event: E) -> EventId {
        let at = self.now.saturating_add(delta);
        // `at >= now` by construction, so this cannot fail.
        self.schedule_at(at, priority, event)
            .unwrap_or_else(|_| unreachable!("schedule_in is never in the past"))
    }

    /// Cancels a pending event. Linear in the queue length; cancellation is rare.
    pub fn cancel(&mut self, id: EventId) -> bool {
        let key = self.queue.keys().find(|k| k.2 == id).copied();
        match key {
            Some(k) => self.queue.remove(&k).is_some(),
            None => false,
        }
    }

    /// Pops the next event if it is due at or before `limit`, advancing `now`
    /// to the event's time.
    pub fn pop_due(&mut self, limit: Ps) -> Option<(Ps, E)> {
        let key = *self.queue.keys().next()?;
        if key.0 > limit {
            return None;
        }
        let event = self.queue.remove(&key)?;
        self.now = key.0;
        Some((key.0, event))
    }

    /// Moves time forward to `t` without firing anything. Does nothing if `t`
    /// is in the past. Callers must have drained events due before `t`.
    pub fn advance_to(&mut self, t: Ps) {
        if t > self.now {
            self.now = t;
        }
    }
}

impl<E: Clone> From<Scheduler<E>> for State<E> {
    fn from(s: Scheduler<E>) -> State<E> {
        State {
            now: s.now,
            next_id: s.next_id,
            entries: s
                .queue
                .into_iter()
                .map(|((at, priority, id), event)| Entry {
                    at,
                    priority,
                    id,
                    event,
                })
                .collect(),
        }
    }
}

impl<E> From<State<E>> for Scheduler<E> {
    fn from(s: State<E>) -> Scheduler<E> {
        Scheduler {
            now: s.now,
            next_id: s.next_id,
            queue: s
                .entries
                .into_iter()
                .map(|e| ((e.at, e.priority, e.id), e.event))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_by_time_then_priority_then_creation() {
        let mut s = Scheduler::new();
        s.schedule_at(Ps(20), 0, "late").unwrap();
        s.schedule_at(Ps(10), 5, "low-prio-first-made").unwrap();
        s.schedule_at(Ps(10), 1, "high-prio").unwrap();
        s.schedule_at(Ps(10), 5, "low-prio-second-made").unwrap();
        let mut order = vec![];
        while let Some((t, e)) = s.pop_due(Ps::MAX) {
            order.push((t.0, e));
        }
        assert_eq!(
            order,
            vec![
                (10, "high-prio"),
                (10, "low-prio-first-made"),
                (10, "low-prio-second-made"),
                (20, "late"),
            ]
        );
    }

    #[test]
    fn pop_due_respects_limit_and_advances_now() {
        let mut s = Scheduler::new();
        s.schedule_at(Ps(100), 0, 1).unwrap();
        assert!(s.pop_due(Ps(99)).is_none());
        assert_eq!(s.now(), Ps::ZERO);
        assert_eq!(s.pop_due(Ps(100)), Some((Ps(100), 1)));
        assert_eq!(s.now(), Ps(100));
    }

    #[test]
    fn rejects_the_past_and_cancels() {
        let mut s = Scheduler::new();
        s.advance_to(Ps(50));
        assert!(matches!(
            s.schedule_at(Ps(49), 0, ()),
            Err(SchedError::InPast { .. })
        ));
        let id = s.schedule_in(Ps(10), 0, ());
        assert!(s.cancel(id));
        assert!(!s.cancel(id));
        assert!(s.is_empty());
    }

    #[test]
    fn serde_round_trip_preserves_order_and_ids() {
        let mut s: Scheduler<u32> = Scheduler::new();
        s.schedule_at(Ps(5), 0, 1).unwrap();
        s.schedule_at(Ps(5), 0, 2).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let mut r: Scheduler<u32> = serde_json::from_str(&json).unwrap();
        // A new event after restore must sort after the restored ones.
        r.schedule_at(Ps(5), 0, 3).unwrap();
        let got: Vec<u32> = std::iter::from_fn(|| r.pop_due(Ps::MAX).map(|(_, e)| e)).collect();
        assert_eq!(got, vec![1, 2, 3]);
    }
}
