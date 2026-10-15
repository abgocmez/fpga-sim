/// 64-bit FNV-1a, used as the canonical trace digest.
///
/// It is not cryptographic and does not need to be: it only detects
/// divergence between runs. It is chosen because it is trivial to specify and
/// re-implement in any language (see `docs/adr/005-trace-hash.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv64(u64);

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

impl Default for Fnv64 {
    fn default() -> Self {
        Fnv64(OFFSET_BASIS)
    }
}

impl Fnv64 {
    /// A fresh hasher.
    pub fn new() -> Fnv64 {
        Fnv64::default()
    }

    /// Absorbs bytes.
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    /// Absorbs a `u64` as 8 little-endian bytes.
    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    /// Absorbs a length-prefixed string so adjacent strings cannot alias.
    pub fn write_str(&mut self, s: &str) {
        self.write_u64(s.len() as u64);
        self.write(s.as_bytes());
    }

    /// The current digest.
    pub fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_fnv1a_vectors() {
        assert_eq!(Fnv64::new().finish(), 0xcbf2_9ce4_8422_2325);
        let mut h = Fnv64::new();
        h.write(b"a");
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
        let mut h = Fnv64::new();
        h.write(b"foobar");
        assert_eq!(h.finish(), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn length_prefix_prevents_aliasing() {
        let mut a = Fnv64::new();
        a.write_str("ab");
        a.write_str("c");
        let mut b = Fnv64::new();
        b.write_str("a");
        b.write_str("bc");
        assert_ne!(a.finish(), b.finish());
    }
}
