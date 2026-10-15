use serde::{Deserialize, Serialize};

/// SplitMix64 pseudo-random generator.
///
/// The whole state is one `u64`, so it snapshots and restores trivially. It is
/// the only source of randomness permitted in the simulator (requirement §4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prng {
    state: u64,
}

impl Prng {
    /// Creates a generator from a seed.
    pub fn new(seed: u64) -> Prng {
        Prng { state: seed }
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `lo..hi` (`hi` exclusive). Returns `lo` when the range is empty.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        // Modulo bias is irrelevant at these range sizes and keeps the result
        // a pure function of the stream.
        lo + self.next_u64() % (hi - lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // Reference values for SplitMix64 seeded with 0.
        let mut p = Prng::new(0);
        assert_eq!(p.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(p.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn same_seed_same_stream_and_snapshot_resumes() {
        let mut a = Prng::new(7);
        let mut b = Prng::new(7);
        for _ in 0..16 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let snap = serde_json::to_string(&a).unwrap();
        let mut c: Prng = serde_json::from_str(&snap).unwrap();
        assert_eq!(a.next_u64(), c.next_u64());
    }

    #[test]
    fn range_bounds() {
        let mut p = Prng::new(1);
        for _ in 0..1000 {
            let v = p.range(10, 20);
            assert!((10..20).contains(&v));
        }
        assert_eq!(p.range(5, 5), 5);
    }
}
