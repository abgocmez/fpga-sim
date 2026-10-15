use crate::time::Ps;
use serde::{Deserialize, Serialize};

/// A named clock domain with a fixed period and phase.
///
/// Domains are first-class (requirement §4): the CPU clock, FCLK0–3 (whose
/// period will follow SLCR at run time), pixel and I2S clocks are all instances
/// of this type. Rising edges occur at `phase + n * period`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockDomain {
    /// Domain name, for example `"FCLK0"`.
    pub name: String,
    /// Clock period; never zero.
    period: Ps,
    /// Time of the first rising edge.
    phase: Ps,
    /// A disabled domain produces no edges.
    pub enabled: bool,
}

impl ClockDomain {
    /// Creates an enabled domain. Returns `None` if `period` is zero.
    pub fn new(name: impl Into<String>, period: Ps, phase: Ps) -> Option<ClockDomain> {
        if period == Ps::ZERO {
            return None;
        }
        Some(ClockDomain {
            name: name.into(),
            period,
            phase,
            enabled: true,
        })
    }

    /// Creates a domain from a frequency in hertz, first edge at time zero.
    pub fn from_hz(name: impl Into<String>, hz: u64) -> Option<ClockDomain> {
        ClockDomain::new(name, Ps::period_of_hz(hz)?, Ps::ZERO)
    }

    /// The clock period.
    pub fn period(&self) -> Ps {
        self.period
    }

    /// Changes the period (for example after an SLCR clock-divider write).
    /// Returns `false` and leaves the domain unchanged if `period` is zero.
    pub fn set_period(&mut self, period: Ps) -> bool {
        if period == Ps::ZERO {
            return false;
        }
        self.period = period;
        true
    }

    /// The first rising edge strictly after `t`, if the domain is enabled.
    pub fn next_edge_after(&self, t: Ps) -> Option<Ps> {
        if !self.enabled {
            return None;
        }
        if t < self.phase {
            return Some(self.phase);
        }
        let n = (t.0 - self.phase.0) / self.period.0 + 1;
        n.checked_mul(self.period.0)
            .and_then(|d| d.checked_add(self.phase.0))
            .map(Ps)
    }

    /// Number of rising edges in the half-open interval `(from, to]`.
    pub fn edges_in(&self, from: Ps, to: Ps) -> u64 {
        if !self.enabled || to <= from {
            return 0;
        }
        self.edges_up_to(to) - self.edges_up_to(from)
    }

    /// Rising edges at times `<= t`.
    fn edges_up_to(&self, t: Ps) -> u64 {
        if t < self.phase {
            0
        } else {
            (t.0 - self.phase.0) / self.period.0 + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_follow_period_and_phase() {
        let c = ClockDomain::new("c", Ps(10), Ps(3)).unwrap();
        assert_eq!(c.next_edge_after(Ps(0)), Some(Ps(3)));
        assert_eq!(c.next_edge_after(Ps(3)), Some(Ps(13)));
        assert_eq!(c.next_edge_after(Ps(12)), Some(Ps(13)));
        // Edges at 3, 13, 23 fall in (0, 25].
        assert_eq!(c.edges_in(Ps(0), Ps(25)), 3);
        assert_eq!(c.edges_in(Ps(3), Ps(13)), 1);
        assert_eq!(c.edges_in(Ps(13), Ps(13)), 0);
    }

    #[test]
    fn rejects_zero_period_and_honours_enable() {
        assert!(ClockDomain::new("c", Ps::ZERO, Ps::ZERO).is_none());
        assert!(ClockDomain::from_hz("c", 0).is_none());
        let mut c = ClockDomain::from_hz("FCLK0", 100_000_000).unwrap();
        assert_eq!(c.period(), Ps(10_000));
        c.enabled = false;
        assert_eq!(c.next_edge_after(Ps(0)), None);
        assert_eq!(c.edges_in(Ps(0), Ps(1_000_000)), 0);
        assert!(!c.set_period(Ps::ZERO));
    }
}
