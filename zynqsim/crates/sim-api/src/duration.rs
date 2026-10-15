use crate::error::ApiError;

/// Parses `"250us"`, `"10ms"`, `"1.5s"` into picoseconds.
///
/// A bare number is rejected: a missing unit is more often a mistake than an
/// intent, and silently reading it as picoseconds would run for no time at all.
pub fn parse_duration(s: &str) -> Result<u64, ApiError> {
    let s = s.trim();
    let split = s
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| bad(s, "missing unit"))?;
    let (num, unit) = s.split_at(split);
    let value: f64 = num.trim().parse().map_err(|_| bad(s, "not a number"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(bad(s, "must be a non-negative number"));
    }
    let mult: f64 = match unit.trim() {
        "ps" => 1.0,
        "ns" => 1e3,
        "us" => 1e6,
        "ms" => 1e9,
        "s" => 1e12,
        other => return Err(bad(s, &format!("unknown unit `{other}`"))),
    };
    let ps = (value * mult).round();
    if ps >= u64::MAX as f64 {
        return Err(bad(s, "too large"));
    }
    Ok(ps as u64)
}

fn bad(s: &str, why: &str) -> ApiError {
    ApiError::invalid_argument(format!("invalid duration `{s}`: {why}"))
        .with_hint("use a number and a unit, for example 250us, 10ms or 1.5s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("250us").unwrap(), 250_000_000);
        assert_eq!(parse_duration("10ms").unwrap(), 10_000_000_000);
        assert_eq!(parse_duration(" 1.5 s ").unwrap(), 1_500_000_000_000);
        assert_eq!(parse_duration("0ps").unwrap(), 0);
        for bad in ["10", "ms", "-1ms", "1 hour", "abc", "", "1e400s"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
