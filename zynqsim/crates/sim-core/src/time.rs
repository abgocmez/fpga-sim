use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// A point or span on the global simulation timeline, in picoseconds.
///
/// A `u64` of picoseconds covers about 213 days of simulated time.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Ps(pub u64);

impl Ps {
    /// Time zero.
    pub const ZERO: Ps = Ps(0);
    /// The largest representable time.
    pub const MAX: Ps = Ps(u64::MAX);

    /// Picoseconds per nanosecond.
    pub const PER_NS: u64 = 1_000;
    /// Picoseconds per microsecond.
    pub const PER_US: u64 = 1_000_000;
    /// Picoseconds per millisecond.
    pub const PER_MS: u64 = 1_000_000_000;
    /// Picoseconds per second.
    pub const PER_S: u64 = 1_000_000_000_000;

    /// `n` nanoseconds (saturating).
    pub const fn from_ns(n: u64) -> Ps {
        Ps(n.saturating_mul(Self::PER_NS))
    }
    /// `n` microseconds (saturating).
    pub const fn from_us(n: u64) -> Ps {
        Ps(n.saturating_mul(Self::PER_US))
    }
    /// `n` milliseconds (saturating).
    pub const fn from_ms(n: u64) -> Ps {
        Ps(n.saturating_mul(Self::PER_MS))
    }
    /// `n` seconds (saturating).
    pub const fn from_s(n: u64) -> Ps {
        Ps(n.saturating_mul(Self::PER_S))
    }
    /// The period of a clock with the given frequency, rounded to the nearest
    /// picosecond. Returns `None` for 0 Hz.
    pub fn period_of_hz(hz: u64) -> Option<Ps> {
        (Self::PER_S + hz / 2).checked_div(hz).map(Ps)
    }
    /// Saturating addition.
    pub const fn saturating_add(self, rhs: Ps) -> Ps {
        Ps(self.0.saturating_add(rhs.0))
    }
    /// Saturating subtraction.
    pub const fn saturating_sub(self, rhs: Ps) -> Ps {
        Ps(self.0.saturating_sub(rhs.0))
    }
    /// The value as seconds, for display only.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / Self::PER_S as f64
    }
}

impl Add for Ps {
    type Output = Ps;
    fn add(self, rhs: Ps) -> Ps {
        Ps(self.0 + rhs.0)
    }
}

impl AddAssign for Ps {
    fn add_assign(&mut self, rhs: Ps) {
        self.0 += rhs.0;
    }
}

impl Sub for Ps {
    type Output = Ps;
    fn sub(self, rhs: Ps) -> Ps {
        Ps(self.0 - rhs.0)
    }
}

impl fmt::Display for Ps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        if v >= Self::PER_S {
            write!(f, "{:.6} s", v as f64 / Self::PER_S as f64)
        } else if v >= Self::PER_MS {
            write!(f, "{:.6} ms", v as f64 / Self::PER_MS as f64)
        } else if v >= Self::PER_US {
            write!(f, "{:.6} us", v as f64 / Self::PER_US as f64)
        } else if v >= Self::PER_NS {
            write!(f, "{:.3} ns", v as f64 / Self::PER_NS as f64)
        } else {
            write!(f, "{v} ps")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_period() {
        assert_eq!(Ps::from_us(2), Ps(2_000_000));
        assert_eq!(Ps::period_of_hz(125_000_000), Some(Ps(8_000)));
        assert_eq!(Ps::period_of_hz(100_000_000), Some(Ps(10_000)));
        assert_eq!(Ps::period_of_hz(0), None);
    }

    #[test]
    fn display_picks_unit() {
        assert_eq!(Ps(999).to_string(), "999 ps");
        assert_eq!(Ps::from_us(1).to_string(), "1.000000 us");
        assert_eq!(Ps::from_s(2).to_string(), "2.000000 s");
    }

    #[test]
    fn json_is_a_bare_number() {
        assert_eq!(serde_json::to_string(&Ps(42)).unwrap(), "42");
    }
}
