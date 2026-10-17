//! Four-valued bit vectors (0, 1, X, Z) of up to 128 bits, and the IEEE 1364 operator
//! tables on them.
//!
//! A [`Value4`] is three masks over the width of the value. `bits` holds the known ones,
//! `unknown` the bits that are X or Z, and `z` the bits that are Z (a subset of `unknown`;
//! an unknown bit that is not Z is X). The representation is kept normalised: `bits` is
//! zero wherever `unknown` is set, and `z` is zero wherever `unknown` is clear. Bits
//! above the width are zero.
//!
//! The operators below are the 4-state tables of IEEE 1364-2005, clause 5 (bitwise,
//! reduction, logical, equality, relational, shift, conditional). Arithmetic is all-X
//! when any operand bit is unknown. Operators turn Z into X, so only the operators that
//! move values without computing on them (selects, concatenation, the conditional merge)
//! keep a Z. The engine's tests check each table against an independent reference.

use std::fmt;

/// Mask of the low `w` bits (`w` may be 128).
pub(crate) fn mask(w: u32) -> u128 {
    if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    }
}

/// One bit of a [`Value4`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bit4 {
    /// Logic 0.
    Zero,
    /// Logic 1.
    One,
    /// Unknown.
    X,
    /// High impedance.
    Z,
}

impl Bit4 {
    /// The character `0`, `1`, `x` or `z` used by `%b` and by VCD.
    pub fn ch(self) -> char {
        match self {
            Bit4::Zero => '0',
            Bit4::One => '1',
            Bit4::X => 'x',
            Bit4::Z => 'z',
        }
    }
}

/// A vector of 0, 1, X and Z bits (see the module documentation for the encoding).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Value4 {
    /// Known ones. Zero wherever `unknown` is set.
    pub bits: u128,
    /// Bits that are X or Z.
    pub unknown: u128,
    /// Bits that are Z. A subset of `unknown`.
    pub z: u128,
}

impl Value4 {
    /// A fully known value (the caller masks it to its width).
    pub const fn known(bits: u128) -> Value4 {
        Value4 {
            bits,
            unknown: 0,
            z: 0,
        }
    }

    /// Every bit of a `w`-bit value is X.
    pub fn all_x(w: u32) -> Value4 {
        Value4 {
            bits: 0,
            unknown: mask(w),
            z: 0,
        }
    }

    /// Every bit of a `w`-bit value is Z.
    pub fn all_z(w: u32) -> Value4 {
        Value4 {
            bits: 0,
            unknown: mask(w),
            z: mask(w),
        }
    }

    /// Masks the value to `w` bits and restores the invariants.
    pub fn norm(self, w: u32) -> Value4 {
        let m = mask(w);
        let unknown = self.unknown & m;
        Value4 {
            bits: self.bits & m & !unknown,
            unknown,
            z: self.z & m & unknown,
        }
    }

    /// Whether no bit is X or Z.
    pub fn is_known(&self) -> bool {
        self.unknown == 0
    }

    /// The state of bit `i` (bits above the value read as X).
    pub fn bit(&self, i: u32) -> Bit4 {
        if i >= 128 {
            return Bit4::X;
        }
        let k = 1u128 << i;
        if self.unknown & k == 0 {
            if self.bits & k != 0 {
                Bit4::One
            } else {
                Bit4::Zero
            }
        } else if self.z & k != 0 {
            Bit4::Z
        } else {
            Bit4::X
        }
    }

    /// The truth value used by `if`, `while`, `?:` and the logical operators: `Some(true)`
    /// when some bit is a known 1, `Some(false)` when every bit is a known 0, and `None`
    /// (unknown) otherwise.
    pub fn truth(&self) -> Option<bool> {
        if self.bits != 0 {
            Some(true)
        } else if self.unknown == 0 {
            Some(false)
        } else {
            None
        }
    }

    /// The 2-state view: X and Z read as 0.
    pub fn to_two_state(self) -> u128 {
        self.bits
    }

    /// The value as `w` characters, most significant first (`0`, `1`, `x`, `z`).
    pub fn to_bin_string(self, w: u32) -> String {
        (0..w).rev().map(|i| self.bit(i).ch()).collect()
    }

    /// Positions the low `w` bits of `v` at `lo` (for a part write).
    pub(crate) fn shifted(self, lo: u32, w: u32) -> Value4 {
        let m = mask(w);
        let sh = |x: u128| if lo >= 128 { 0 } else { (x & m) << lo };
        Value4 {
            bits: sh(self.bits),
            unknown: sh(self.unknown),
            z: sh(self.z),
        }
    }

    /// Bits `lo .. lo + w` of the value, moved to position 0. Bits above the value read
    /// as X (the caller decides which ranges are in bounds).
    pub(crate) fn part(self, lo: u32, w: u32) -> Value4 {
        let m = mask(w);
        let sh = |x: u128| if lo >= 128 { 0 } else { (x >> lo) & m };
        Value4 {
            bits: sh(self.bits),
            unknown: sh(self.unknown),
            z: sh(self.z),
        }
        .norm(w)
    }

    /// Replaces bits `lo .. lo + w` of `self` with the low `w` bits of `v`.
    pub(crate) fn insert(self, lo: u32, w: u32, v: Value4) -> Value4 {
        let m = if lo >= 128 { 0 } else { mask(w) << lo };
        let s = v.shifted(lo, w);
        Value4 {
            bits: (self.bits & !m) | (s.bits & m),
            unknown: (self.unknown & !m) | (s.unknown & m),
            z: (self.z & !m) | (s.z & m),
        }
    }

    /// Concatenation step: `self` followed (on the right) by `part` of width `pw`.
    pub(crate) fn push(self, part: Value4, pw: u32) -> Value4 {
        let sh = |x: u128| if pw >= 128 { 0 } else { x << pw };
        Value4 {
            bits: sh(self.bits) | part.bits,
            unknown: sh(self.unknown) | part.unknown,
            z: sh(self.z) | part.z,
        }
    }
}

impl fmt::Display for Value4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "bits={:#x} unknown={:#x} z={:#x}",
            self.bits, self.unknown, self.z
        )
    }
}

/// Digits of radix `2^bpd` (lower case, no underscores) as an `lw`-bit value. `x` digits
/// are unknown, `z` and `?` digits are Z. A number shorter than `lw` bits is extended
/// with its leftmost digit's state when that is X or Z, and with zeros otherwise.
pub(crate) fn radix_bits(digits: &str, bpd: u32, lw: u32) -> Value4 {
    let full = mask(bpd);
    let mut out = Value4::default();
    let mut total = 0u32;
    for c in digits.chars() {
        let part = match c {
            'x' => Value4 {
                bits: 0,
                unknown: full,
                z: 0,
            },
            'z' | '?' => Value4 {
                bits: 0,
                unknown: full,
                z: full,
            },
            d => Value4::known(d.to_digit(1 << bpd).unwrap_or(0) as u128),
        };
        out = out.push(part, bpd);
        total += bpd;
    }
    if total < lw {
        let fill = mask(lw) & !mask(total);
        match digits.chars().next() {
            Some('x') => out.unknown |= fill,
            Some('z' | '?') => {
                out.unknown |= fill;
                out.z |= fill;
            }
            _ => {}
        }
    }
    out.norm(lw)
}

/// The value a set of drivers gives a net of `w` bits. Each driver is its value and the
/// bits it drives. Z does not drive, equal known values keep, and a conflict or an X
/// gives X; a bit that no driver drives is Z.
pub(crate) fn resolve(drivers: impl Iterator<Item = (Value4, u128)>, w: u32) -> Value4 {
    let m = mask(w);
    let (mut h1, mut h0, mut hx) = (0u128, 0u128, 0u128);
    for (v, cover) in drivers {
        let c = cover & m;
        h1 |= v.bits & c;
        h0 |= c & !(v.bits | v.unknown);
        hx |= c & v.unknown & !v.z;
    }
    let zres = m & !(h1 | h0 | hx);
    let xres = hx | (h1 & h0);
    Value4 {
        bits: h1 & !(h0 | hx) & m,
        unknown: zres | xres,
        z: zres,
    }
}

/// Bits known to be 0 within `m`.
fn known0(a: Value4, m: u128) -> u128 {
    m & !a.bits & !a.unknown
}

/// Bitwise AND (`&`).
pub(crate) fn and(a: Value4, b: Value4, m: u128) -> Value4 {
    let one = a.bits & b.bits;
    let zero = known0(a, m) | known0(b, m);
    Value4 {
        bits: one,
        unknown: m & !(one | zero),
        z: 0,
    }
}

/// Bitwise OR (`|`).
pub(crate) fn or(a: Value4, b: Value4, m: u128) -> Value4 {
    let one = a.bits | b.bits;
    let zero = known0(a, m) & known0(b, m);
    Value4 {
        bits: one,
        unknown: m & !(one | zero),
        z: 0,
    }
}

/// Bitwise XOR (`^`).
pub(crate) fn xor(a: Value4, b: Value4, m: u128) -> Value4 {
    let unk = (a.unknown | b.unknown) & m;
    Value4 {
        bits: (a.bits ^ b.bits) & m & !unk,
        unknown: unk,
        z: 0,
    }
}

/// Bitwise NOT (`~`): Z becomes X.
pub(crate) fn not(a: Value4, m: u128) -> Value4 {
    Value4 {
        bits: known0(a, m),
        unknown: a.unknown & m,
        z: 0,
    }
}

/// Reduction AND: 0 if any bit is a known 0, X if any is unknown, else 1.
pub(crate) fn reduce_and(a: Value4, w: u32) -> Value4 {
    let m = mask(w);
    if known0(a, m) != 0 {
        Value4::known(0)
    } else if a.unknown & m != 0 {
        Value4::all_x(1)
    } else {
        Value4::known(1)
    }
}

/// Reduction OR: 1 if any bit is a known 1, X if any is unknown, else 0.
pub(crate) fn reduce_or(a: Value4, w: u32) -> Value4 {
    let m = mask(w);
    if a.bits & m != 0 {
        Value4::known(1)
    } else if a.unknown & m != 0 {
        Value4::all_x(1)
    } else {
        Value4::known(0)
    }
}

/// Reduction XOR: X if any bit is unknown, else the parity.
pub(crate) fn reduce_xor(a: Value4, w: u32) -> Value4 {
    if a.unknown & mask(w) != 0 {
        Value4::all_x(1)
    } else {
        Value4::known((a.bits & mask(w)).count_ones() as u128 & 1)
    }
}

/// Logical negation (`!`) of a value of any width.
pub(crate) fn lnot(a: Value4) -> Value4 {
    match a.truth() {
        Some(t) => Value4::known(!t as u128),
        None => Value4::all_x(1),
    }
}

/// Logical AND (`&&`).
pub(crate) fn land(a: Value4, b: Value4) -> Value4 {
    match (a.truth(), b.truth()) {
        (Some(false), _) | (_, Some(false)) => Value4::known(0),
        (Some(true), Some(true)) => Value4::known(1),
        _ => Value4::all_x(1),
    }
}

/// Logical OR (`||`).
pub(crate) fn lor(a: Value4, b: Value4) -> Value4 {
    match (a.truth(), b.truth()) {
        (Some(true), _) | (_, Some(true)) => Value4::known(1),
        (Some(false), Some(false)) => Value4::known(0),
        _ => Value4::all_x(1),
    }
}

/// Logical equality (`==`, `!=` negated by the caller): X when either operand has an
/// unknown bit, else whether the bits are equal.
pub(crate) fn logical_eq(a: Value4, b: Value4, m: u128) -> Value4 {
    if (a.unknown | b.unknown) & m != 0 {
        Value4::all_x(1)
    } else {
        Value4::known(((a.bits ^ b.bits) & m == 0) as u128)
    }
}

/// Case equality (`===`): every bit, X and Z included, is equal.
pub(crate) fn case_eq(a: Value4, b: Value4, m: u128) -> bool {
    ((a.bits ^ b.bits) | (a.unknown ^ b.unknown) | (a.z ^ b.z)) & m == 0
}

/// How a `case` item compares with the selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    /// `case`: exact 4-state equality (`===`).
    Exact,
    /// `casez`: Z in either side is a don't-care.
    Z,
    /// `casex`: X and Z in either side are don't-cares.
    Xz,
}

/// Whether a `case` item matches the selector.
pub(crate) fn case_match(a: Value4, b: Value4, m: u128, mode: CaseMode) -> bool {
    let dont = match mode {
        CaseMode::Exact => 0,
        CaseMode::Z => (a.z | b.z) & m,
        CaseMode::Xz => (a.unknown | b.unknown) & m,
    };
    case_eq(a, b, m & !dont)
}

/// The conditional operator with an unknown condition: a bit is kept when both branches
/// agree on a known value, Z when both are Z, and X otherwise.
pub(crate) fn merge(t: Value4, f: Value4, m: u128) -> Value4 {
    let same = m & !(t.unknown | f.unknown) & !(t.bits ^ f.bits);
    Value4 {
        bits: same & t.bits,
        unknown: m & !same,
        z: t.z & f.z & m,
    }
}

/// Zero or sign extension of `a` (`from` bits) to `to` bits.
pub(crate) fn extend(a: Value4, from: u32, to: u32, signed: bool) -> Value4 {
    let a = a.norm(from);
    if from == 0 || to <= from {
        return a.norm(to);
    }
    let high = mask(to) & !mask(from);
    let mut r = a;
    if signed {
        match a.bit(from - 1) {
            Bit4::One => r.bits |= high,
            Bit4::X => r.unknown |= high,
            Bit4::Z => {
                r.unknown |= high;
                r.z |= high;
            }
            Bit4::Zero => {}
        }
    }
    r.norm(to)
}

/// Logical left shift by `n` (`n` is a known amount).
pub(crate) fn shl(a: Value4, n: u32, m: u128) -> Value4 {
    if n >= 128 {
        return Value4::known(0);
    }
    Value4 {
        bits: (a.bits << n) & m,
        unknown: (a.unknown << n) & m,
        z: 0,
    }
}

/// Logical right shift by `n`.
pub(crate) fn shr(a: Value4, n: u32, w: u32) -> Value4 {
    let m = mask(w);
    if n >= 128 {
        return Value4::known(0);
    }
    Value4 {
        bits: (a.bits >> n) & m,
        unknown: (a.unknown >> n) & m,
        z: 0,
    }
}

/// Arithmetic right shift by `n` of a `w`-bit value: the high bits take the sign bit's
/// state (X or Z when the sign bit is unknown).
pub(crate) fn ashr(a: Value4, n: u32, w: u32) -> Value4 {
    let mut r = shr(a, n, w);
    if n == 0 || w == 0 {
        return r;
    }
    let fill_from = w.saturating_sub(n);
    let fill = mask(w) & !mask(fill_from);
    match a.bit(w - 1) {
        Bit4::One => r.bits |= fill,
        Bit4::X | Bit4::Z => r.unknown |= fill,
        Bit4::Zero => {}
    }
    r.norm(w)
}

/// Arithmetic on `w`-bit operands: all X when an operand has an unknown bit, otherwise
/// `f` applied to the known bits.
pub(crate) fn arith(a: Value4, b: Value4, m: u128, f: impl FnOnce(u128, u128) -> u128) -> Value4 {
    if (a.unknown | b.unknown) != 0 {
        Value4 {
            bits: 0,
            unknown: m,
            z: 0,
        }
    } else {
        Value4::known(f(a.bits, b.bits) & m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value4 {
        let mut out = Value4::default();
        for c in s.chars() {
            out.bits <<= 1;
            out.unknown <<= 1;
            out.z <<= 1;
            match c {
                '0' => {}
                '1' => out.bits |= 1,
                'x' => out.unknown |= 1,
                'z' => {
                    out.unknown |= 1;
                    out.z |= 1;
                }
                _ => panic!("bad char"),
            }
        }
        out
    }

    #[test]
    fn bitwise_tables_match_ieee() {
        let m = 1;
        // AND: 0 dominates, 1 & x = x, z acts as x.
        assert_eq!(and(v("0"), v("x"), m), v("0"));
        assert_eq!(and(v("1"), v("x"), m), v("x"));
        assert_eq!(and(v("1"), v("z"), m), v("x"));
        assert_eq!(and(v("x"), v("x"), m), v("x"));
        // OR: 1 dominates.
        assert_eq!(or(v("1"), v("x"), m), v("1"));
        assert_eq!(or(v("0"), v("x"), m), v("x"));
        assert_eq!(or(v("0"), v("z"), m), v("x"));
        // XOR: any unknown gives X.
        assert_eq!(xor(v("1"), v("x"), m), v("x"));
        assert_eq!(xor(v("1"), v("0"), m), v("1"));
        // NOT: z becomes x.
        assert_eq!(not(v("z"), m), v("x"));
        assert_eq!(not(v("x"), m), v("x"));
        assert_eq!(not(v("0"), m), v("1"));
    }

    #[test]
    fn normalisation_and_bit_states() {
        let a = v("1xz0").norm(4);
        assert_eq!(a.bit(3), Bit4::One);
        assert_eq!(a.bit(2), Bit4::X);
        assert_eq!(a.bit(1), Bit4::Z);
        assert_eq!(a.bit(0), Bit4::Zero);
        assert_eq!(a.to_bin_string(4), "1xz0");
        let junk = Value4 {
            bits: 0xff,
            unknown: 0x0f,
            z: 0xf0,
        }
        .norm(4);
        // Bits above the width are dropped; unknown bits carry no value; z only where unknown.
        assert_eq!(junk.bits, 0);
        assert_eq!(junk.unknown, 0x0f);
        assert_eq!(junk.z, 0);
    }

    #[test]
    fn truth_of_vectors() {
        assert_eq!(v("0x0").truth(), None);
        assert_eq!(v("1x0").truth(), Some(true));
        assert_eq!(v("000").truth(), Some(false));
        assert_eq!(v("zz").truth(), None);
    }

    #[test]
    fn merge_and_extend() {
        let m = 0b11;
        // Equal known bits survive an unknown condition, others become x.
        let r = merge(v("10"), v("11"), m);
        assert_eq!(r.to_bin_string(2), "1x");
        let r = merge(v("zz"), v("zz"), m);
        assert_eq!(r.to_bin_string(2), "zz");
        // Sign extension of an unknown sign bit extends the unknown.
        let e = extend(v("x0"), 2, 4, true);
        assert_eq!(e.to_bin_string(4), "xxx0");
        let e = extend(v("x0"), 2, 4, false);
        assert_eq!(e.to_bin_string(4), "00x0");
    }
}
