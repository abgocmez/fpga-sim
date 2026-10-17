//! An independent reference for the VFP arithmetic, for the property tests.
//!
//! It does not share code with the interpreter. Exact values are rationals over a small
//! bignum, and a rounding result is accepted only if it lies in the rounding interval of the
//! exact value, which is decided by exact comparisons (no division, no shared rounding routine).
//! The conventions are those of ADR-030: tininess before rounding, FZ flushes inputs and
//! results that round to a subnormal, default NaN with DN, and NaN operands in operand order.

#![allow(dead_code)]

use std::cmp::Ordering;

// ---------------------------------------------------------------- bignum

/// A nonnegative integer, little-endian 32-bit limbs, no high zero limbs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big(Vec<u32>);

impl Big {
    fn trim(mut v: Vec<u32>) -> Big {
        while v.last() == Some(&0) {
            v.pop();
        }
        Big(v)
    }
    pub fn from_u128(v: u128) -> Big {
        Big::trim((0..4).map(|i| (v >> (32 * i)) as u32).collect())
    }
    pub fn pow2(k: u32) -> Big {
        let mut v = vec![0u32; (k / 32) as usize];
        v.push(1u32 << (k % 32));
        Big(v)
    }
    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
    pub fn mul(&self, o: &Big) -> Big {
        let mut r = vec![0u64; self.0.len() + o.0.len() + 1];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                let t = r[i + j] + u64::from(a) * u64::from(b) + carry;
                r[i + j] = t & 0xFFFF_FFFF;
                carry = t >> 32;
            }
            let mut k = i + o.0.len();
            while carry != 0 {
                let t = r[k] + carry;
                r[k] = t & 0xFFFF_FFFF;
                carry = t >> 32;
                k += 1;
            }
        }
        Big::trim(r.into_iter().map(|x| x as u32).collect())
    }
    pub fn add(&self, o: &Big) -> Big {
        let n = self.0.len().max(o.0.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let s = u64::from(*self.0.get(i).unwrap_or(&0))
                + u64::from(*o.0.get(i).unwrap_or(&0))
                + carry;
            out.push(s as u32);
            carry = s >> 32;
        }
        out.push(carry as u32);
        Big::trim(out)
    }
    /// `self - o`; the caller guarantees `self >= o`.
    pub fn sub(&self, o: &Big) -> Big {
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0i64;
        for i in 0..self.0.len() {
            let mut d = i64::from(self.0[i]) - i64::from(*o.0.get(i).unwrap_or(&0)) - borrow;
            borrow = 0;
            if d < 0 {
                d += 1 << 32;
                borrow = 1;
            }
            out.push(d as u32);
        }
        assert_eq!(borrow, 0, "bignum subtraction underflow");
        Big::trim(out)
    }
    pub fn cmp_big(&self, o: &Big) -> Ordering {
        if self.0.len() != o.0.len() {
            return self.0.len().cmp(&o.0.len());
        }
        for i in (0..self.0.len()).rev() {
            if self.0[i] != o.0[i] {
                return self.0[i].cmp(&o.0[i]);
            }
        }
        Ordering::Equal
    }
}

// ---------------------------------------------------------------- rationals

/// A nonnegative rational `n / d` with `d > 0`.
#[derive(Clone, Debug)]
pub struct Q {
    pub n: Big,
    pub d: Big,
}

impl Q {
    pub fn int(v: u128) -> Q {
        Q {
            n: Big::from_u128(v),
            d: Big::pow2(0),
        }
    }
    /// `m * 2^e`.
    pub fn dyadic(m: u128, e: i64) -> Q {
        let m = Big::from_u128(m);
        if e >= 0 {
            Q {
                n: m.mul(&Big::pow2(e as u32)),
                d: Big::pow2(0),
            }
        } else {
            Q {
                n: m,
                d: Big::pow2((-e) as u32),
            }
        }
    }
    pub fn mul(&self, o: &Q) -> Q {
        Q {
            n: self.n.mul(&o.n),
            d: self.d.mul(&o.d),
        }
    }
    pub fn div(&self, o: &Q) -> Q {
        Q {
            n: self.n.mul(&o.d),
            d: self.d.mul(&o.n),
        }
    }
    /// `self + o` (both nonnegative).
    pub fn add(&self, o: &Q) -> Q {
        Q {
            n: self.n.mul(&o.d).add(&o.n.mul(&self.d)),
            d: self.d.mul(&o.d),
        }
    }
    /// `self - o` for `self >= o`.
    pub fn sub(&self, o: &Q) -> Q {
        Q {
            n: self.n.mul(&o.d).sub(&o.n.mul(&self.d)),
            d: self.d.mul(&o.d),
        }
    }
    pub fn cmp_q(&self, o: &Q) -> Ordering {
        self.n.mul(&o.d).cmp_big(&o.n.mul(&self.d))
    }
    pub fn is_zero(&self) -> bool {
        self.n.is_zero()
    }
    /// `(a + b) / 2`.
    fn mid(a: &Q, b: &Q) -> Q {
        let sum = Q {
            n: a.n.mul(&b.d).add(&b.n.mul(&a.d)),
            d: a.d.mul(&b.d),
        };
        // (a + b) / 2 = sum.n / (2 * sum.d).
        Q {
            n: sum.n,
            d: sum.d.add(&sum.d),
        }
    }
}

// ---------------------------------------------------------------- formats

/// A binary format: exponent and fraction widths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct F {
    pub ebits: u32,
    pub fbits: u32,
}

pub const SINGLE: F = F {
    ebits: 8,
    fbits: 23,
};
pub const DOUBLE: F = F {
    ebits: 11,
    fbits: 52,
};

impl F {
    pub fn bias(self) -> i64 {
        (1i64 << (self.ebits - 1)) - 1
    }
    pub fn emin(self) -> i64 {
        1 - self.bias()
    }
    pub fn sign_shift(self) -> u32 {
        self.ebits + self.fbits
    }
    pub fn exp_all_ones(self) -> u64 {
        (1u64 << self.ebits) - 1
    }
    /// Magnitude bits of infinity.
    pub fn inf_mag(self) -> u64 {
        self.exp_all_ones() << self.fbits
    }
    /// Magnitude bits of the largest finite value.
    pub fn max_mag(self) -> u64 {
        self.inf_mag() - 1
    }
    pub fn frac_mask(self) -> u64 {
        (1u64 << self.fbits) - 1
    }
    pub fn quiet_bit(self) -> u64 {
        1u64 << (self.fbits - 1)
    }
    pub fn default_nan(self) -> u64 {
        self.inf_mag() | self.quiet_bit()
    }
    pub fn sign(self, v: u64) -> u64 {
        (v >> self.sign_shift()) & 1
    }
    pub fn mag(self, v: u64) -> u64 {
        v & ((1u64 << self.sign_shift()) - 1)
    }
    pub fn with_sign(self, mag: u64, sign: u64) -> u64 {
        mag | (sign << self.sign_shift())
    }
}

/// The exact value of a magnitude (finite, nonzero or zero) of format `f`.
pub fn lattice(f: F, mag: u64) -> Q {
    let e = mag >> f.fbits;
    let frac = u128::from(mag & f.frac_mask());
    if e == f.exp_all_ones() {
        // Infinity stands for 2^(emax+1), the next value above the largest finite one.
        return Q::dyadic(1, f.bias() + 1);
    }
    if e == 0 {
        Q::dyadic(frac, f.emin() - i64::from(f.fbits))
    } else {
        Q::dyadic(
            frac | (1u128 << f.fbits),
            e as i64 - f.bias() - i64::from(f.fbits),
        )
    }
}

// ---------------------------------------------------------------- rounding predicates

/// A value to be rounded: an exact rational or the square root of one.
#[derive(Clone, Debug)]
pub enum Target {
    Ratio(Q),
    Sqrt(Q),
}

impl Target {
    /// Compares the exact (unrounded) value with the rational point `p`.
    pub fn cmp_point(&self, p: &Q) -> Ordering {
        match self {
            Target::Ratio(v) => v.cmp_point_q(p),
            // sqrt(x) ? p  is  x ? p^2 for p >= 0.
            Target::Sqrt(x) => {
                let p2 = Q {
                    n: p.n.mul(&p.n),
                    d: p.d.mul(&p.d),
                };
                x.cmp_q(&p2)
            }
        }
    }
    fn is_pos(&self) -> bool {
        match self {
            Target::Ratio(v) | Target::Sqrt(v) => !v.is_zero(),
        }
    }
}

impl Q {
    fn cmp_point_q(&self, p: &Q) -> Ordering {
        self.cmp_q(p)
    }
}

/// The direction the magnitude is rounded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Nearest,
    Up,
    Down,
}

/// The magnitude direction for FPSCR.RMode `rm` (0 nearest, 1 +inf, 2 -inf, 3 zero) and sign.
pub fn dir_of(rm: u32, sign: u64) -> Dir {
    match (rm & 3, sign) {
        (0, _) => Dir::Nearest,
        (3, _) => Dir::Down,
        (1, 0) | (2, 1) => Dir::Up,
        _ => Dir::Down,
    }
}

/// Whether magnitude `c` is a correct rounding of the target in format `f` under `dir`,
/// with the value rounded for an unbounded exponent range and then limited (overflow to the
/// largest finite value for the directed modes that round toward zero).
pub fn correct(f: F, t: &Target, dir: Dir, c: u64) -> bool {
    let inf = f.inf_mag();
    let max = f.max_mag();
    if c == 0 {
        let hi = lattice(f, 1);
        return match dir {
            Dir::Nearest => t.cmp_point(&Q::mid(&Q::int(0), &hi)) != Ordering::Greater,
            Dir::Down => t.cmp_point(&hi) == Ordering::Less,
            Dir::Up => false,
        };
    }
    if c == inf {
        return match dir {
            Dir::Nearest => {
                t.cmp_point(&Q::mid(&lattice(f, max), &lattice(f, inf))) != Ordering::Less
            }
            Dir::Up => t.cmp_point(&lattice(f, max)) == Ordering::Greater,
            Dir::Down => false,
        };
    }
    let lo = lattice(f, c);
    let hi = lattice(f, c + 1);
    let pr = lattice(f, c - 1);
    let even = c & 1 == 0;
    match dir {
        Dir::Nearest => {
            let low_ok = match t.cmp_point(&Q::mid(&pr, &lo)) {
                Ordering::Greater => true,
                Ordering::Equal => even,
                Ordering::Less => false,
            };
            let high_ok = match t.cmp_point(&Q::mid(&lo, &hi)) {
                Ordering::Less => true,
                Ordering::Equal => even,
                Ordering::Greater => false,
            };
            low_ok && high_ok
        }
        Dir::Down => {
            t.cmp_point(&lo) != Ordering::Less && (c == max || t.cmp_point(&hi) == Ordering::Less)
        }
        Dir::Up => t.cmp_point(&pr) == Ordering::Greater && t.cmp_point(&lo) != Ordering::Greater,
    }
}

fn inexact(f: F, t: &Target, c: u64) -> bool {
    c == f.inf_mag() || t.cmp_point(&lattice(f, c)) != Ordering::Equal
}

/// The exact value is below the smallest normal number (tininess before rounding).
fn tiny(f: F, t: &Target) -> bool {
    t.cmp_point(&lattice(f, 1 << f.fbits)) == Ordering::Less
}

/// The result, rounded with an unbounded exponent, exceeds the largest finite value.
fn overflows(f: F, t: &Target, dir: Dir) -> bool {
    let inf_v = lattice(f, f.inf_mag());
    match dir {
        Dir::Nearest => t.cmp_point(&Q::mid(&lattice(f, f.max_mag()), &inf_v)) != Ordering::Less,
        Dir::Up => t.cmp_point(&lattice(f, f.max_mag())) == Ordering::Greater,
        Dir::Down => t.cmp_point(&inf_v) != Ordering::Less,
    }
}

/// With FZ, the correctly rounded (subnormal-range) result is a subnormal or zero.
fn subnormal_result(f: F, t: &Target, dir: Dir) -> bool {
    let max_sub = lattice(f, (1 << f.fbits) - 1);
    let min_normal = lattice(f, 1 << f.fbits);
    match dir {
        Dir::Nearest => {
            let mid = Q::mid(&max_sub, &min_normal);
            t.cmp_point(&mid) == Ordering::Less
        }
        Dir::Up => t.cmp_point(&max_sub) != Ordering::Greater,
        Dir::Down => t.cmp_point(&min_normal) == Ordering::Less,
    }
}

pub const IOC: u32 = 1;
pub const DZC: u32 = 2;
pub const OFC: u32 = 4;
pub const UFC: u32 = 8;
pub const IXC: u32 = 16;
pub const IDC: u32 = 128;

/// What an operation must produce.
#[derive(Clone, Debug)]
pub enum Exp {
    /// A known result and flags (specials and NaNs).
    Exact { bits: u64, flags: u32 },
    /// A finite nonzero exact value (`sign`, target) to be correctly rounded.
    Round { t: Target, sign: u64, in_flags: u32 },
}

/// Checks the machine's result against an expectation. `rm` is the FPSCR.RMode index, `fz`
/// the FZ setting.
pub fn verify(
    exp: &Exp,
    f: F,
    rm: u32,
    fz: bool,
    got_bits: u64,
    got_flags: u32,
) -> Result<(), String> {
    let got_flags = got_flags & 0x9F;
    match exp {
        Exp::Exact { bits, flags } => {
            if got_bits != *bits || got_flags != *flags {
                return Err(format!(
                    "exact: got {got_bits:#x} flags {got_flags:#x}, want {bits:#x} flags {flags:#x}"
                ));
            }
            Ok(())
        }
        Exp::Round { t, sign, in_flags } => {
            let dir = dir_of(rm, *sign);
            let mut want = *in_flags;
            if fz && subnormal_result(f, t, dir) {
                want |= UFC | IXC;
                if got_bits != f.with_sign(0, *sign) {
                    return Err(format!("FZ flush: got {got_bits:#x}"));
                }
            } else {
                if f.sign(got_bits) != *sign {
                    return Err(format!("sign: got {got_bits:#x}"));
                }
                let c = f.mag(got_bits);
                if !correct(f, t, dir, c) {
                    return Err(format!("not the correct rounding: got {got_bits:#x}"));
                }
                if inexact(f, t, c) {
                    want |= IXC;
                    if tiny(f, t) {
                        want |= UFC;
                    }
                }
                if overflows(f, t, dir) {
                    want |= OFC | IXC;
                }
            }
            if got_flags != want {
                return Err(format!("flags: got {got_flags:#x}, want {want:#x}"));
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- operands

/// The class of an operand (after input flushing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Nan { signaling: bool },
    Inf(u64),
    Zero(u64),
    Fin(u64),
}

pub fn kind(f: F, v: u64) -> Kind {
    let mag = f.mag(v);
    let sign = f.sign(v);
    let e = mag >> f.fbits;
    let frac = mag & f.frac_mask();
    if e == f.exp_all_ones() {
        if frac == 0 {
            Kind::Inf(sign)
        } else {
            Kind::Nan {
                signaling: frac & f.quiet_bit() == 0,
            }
        }
    } else if mag == 0 {
        Kind::Zero(sign)
    } else {
        Kind::Fin(sign)
    }
}

fn is_subnormal(f: F, v: u64) -> bool {
    let mag = f.mag(v);
    mag != 0 && (mag >> f.fbits) == 0
}

/// Input flushing: with FZ a subnormal is a signed zero and raises IDC.
fn flush(f: F, v: u64, fz: bool, in_flags: &mut u32) -> u64 {
    if fz && is_subnormal(f, v) {
        *in_flags |= IDC;
        f.with_sign(0, f.sign(v))
    } else {
        v
    }
}

/// The NaN result of an operation with NaN operands (in operand order).
fn nan_result(f: F, ops: &[u64], dn: bool) -> Exp {
    let is_nan = |v: u64| matches!(kind(f, v), Kind::Nan { .. });
    let first_s = ops
        .iter()
        .copied()
        .find(|&v| kind(f, v) == Kind::Nan { signaling: true });
    let first = first_s.or_else(|| ops.iter().copied().find(|&v| is_nan(v)));
    let flags = if first_s.is_some() { IOC } else { 0 };
    let bits = match (dn, first) {
        (false, Some(v)) => f.mag(v) | f.quiet_bit() | (f.sign(v) << f.sign_shift()),
        _ => f.default_nan(),
    };
    Exp::Exact { bits, flags }
}

/// The operations of the binary group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bin {
    Add,
    Sub,
    Mul,
    Div,
}

/// The expected result of `a op b` (or `sqrt(a)` for `Bin` not used: see `expect_sqrt`).
pub fn expect_binary(op: Bin, f: F, a: u64, b: u64, fz: bool, dn: bool, rm: u32) -> Exp {
    let mut in_flags = 0;
    let a = flush(f, a, fz, &mut in_flags);
    let b = flush(f, b, fz, &mut in_flags);
    if matches!(kind(f, a), Kind::Nan { .. }) || matches!(kind(f, b), Kind::Nan { .. }) {
        return match nan_result(f, &[a, b], dn) {
            Exp::Exact { bits, flags } => Exp::Exact {
                bits,
                flags: flags | in_flags,
            },
            other => other,
        };
    }
    let ex = |bits: u64, flags: u32| Exp::Exact {
        bits,
        flags: flags | in_flags,
    };
    // For subtraction the second operand's sign is flipped.
    let sb = if op == Bin::Sub {
        f.sign(b) ^ 1
    } else {
        f.sign(b)
    };
    let (ka, kb) = (kind(f, a), kind(f, b));
    let kb = match kb {
        Kind::Inf(_) => Kind::Inf(sb),
        Kind::Zero(_) => Kind::Zero(sb),
        Kind::Fin(_) => Kind::Fin(sb),
        other => other,
    };
    let invalid = || ex(f.default_nan(), IOC);
    let qa = lattice(f, f.mag(a));
    let qb = lattice(f, f.mag(b));
    let inf_bits = |s: u64| f.with_sign(f.inf_mag(), s);
    let zero_bits = |s: u64| f.with_sign(0, s);
    match op {
        Bin::Add | Bin::Sub => match (ka, kb) {
            (Kind::Inf(x), Kind::Inf(y)) => {
                if x == y {
                    ex(inf_bits(x), 0)
                } else {
                    invalid()
                }
            }
            (Kind::Inf(s), _) | (_, Kind::Inf(s)) => ex(inf_bits(s), 0),
            (Kind::Zero(x), Kind::Zero(y)) => {
                let s = if x == y { x } else { u64::from(rm & 3 == 2) };
                ex(zero_bits(s), 0)
            }
            (Kind::Zero(_), Kind::Fin(sy)) => ex(f.with_sign(f.mag(b), sy), 0),
            (Kind::Fin(sx), Kind::Zero(_)) => ex(f.with_sign(f.mag(a), sx), 0),
            (Kind::Fin(sx), Kind::Fin(sy)) => {
                if sx == sy {
                    Exp::Round {
                        t: Target::Ratio(qa.add(&qb)),
                        sign: sx,
                        in_flags,
                    }
                } else {
                    match qa.cmp_q(&qb) {
                        Ordering::Equal => ex(zero_bits(u64::from(rm & 3 == 2)), 0),
                        Ordering::Greater => Exp::Round {
                            t: Target::Ratio(qa.sub(&qb)),
                            sign: sx,
                            in_flags,
                        },
                        Ordering::Less => Exp::Round {
                            t: Target::Ratio(qb.sub(&qa)),
                            sign: sy_of(&kb),
                            in_flags,
                        },
                    }
                }
            }
            _ => invalid(),
        },
        Bin::Mul => match (ka, kb) {
            (Kind::Inf(_), Kind::Zero(_)) | (Kind::Zero(_), Kind::Inf(_)) => invalid(),
            (Kind::Inf(x), Kind::Inf(y))
            | (Kind::Inf(x), Kind::Fin(y))
            | (Kind::Fin(x), Kind::Inf(y)) => ex(inf_bits(x ^ y), 0),
            (Kind::Zero(x), Kind::Zero(y))
            | (Kind::Zero(x), Kind::Fin(y))
            | (Kind::Fin(x), Kind::Zero(y)) => ex(zero_bits(x ^ y), 0),
            (Kind::Fin(x), Kind::Fin(y)) => Exp::Round {
                t: Target::Ratio(qa.mul(&qb)),
                sign: x ^ y,
                in_flags,
            },
            _ => invalid(),
        },
        Bin::Div => match (ka, kb) {
            (Kind::Inf(_), Kind::Inf(_)) | (Kind::Zero(_), Kind::Zero(_)) => invalid(),
            (Kind::Inf(x), Kind::Fin(y)) | (Kind::Inf(x), Kind::Zero(y)) => ex(inf_bits(x ^ y), 0),
            (Kind::Fin(x), Kind::Inf(y))
            | (Kind::Zero(x), Kind::Inf(y))
            | (Kind::Zero(x), Kind::Fin(y)) => ex(zero_bits(x ^ y), 0),
            (Kind::Fin(x), Kind::Zero(y)) => ex(inf_bits(x ^ y), DZC),
            (Kind::Fin(x), Kind::Fin(y)) => Exp::Round {
                t: Target::Ratio(qa.div(&qb)),
                sign: x ^ y,
                in_flags,
            },
            _ => invalid(),
        },
    }
}

fn sy_of(k: &Kind) -> u64 {
    match k {
        Kind::Fin(s) => *s,
        _ => 0,
    }
}

/// The expected result of `sqrt(a)`.
pub fn expect_sqrt(f: F, a: u64, fz: bool, dn: bool) -> Exp {
    let mut in_flags = 0;
    let a = flush(f, a, fz, &mut in_flags);
    let ex = |bits: u64, flags: u32| Exp::Exact {
        bits,
        flags: flags | in_flags,
    };
    match kind(f, a) {
        Kind::Nan { .. } => match nan_result(f, &[a], dn) {
            Exp::Exact { bits, flags } => ex(bits, flags),
            other => other,
        },
        Kind::Inf(0) => ex(f.inf_mag(), 0),
        Kind::Inf(_) => ex(f.default_nan(), IOC),
        Kind::Zero(s) => ex(f.with_sign(0, s), 0),
        Kind::Fin(1) => ex(f.default_nan(), IOC),
        Kind::Fin(_) => Exp::Round {
            t: Target::Sqrt(lattice(f, f.mag(a))),
            sign: 0,
            in_flags,
        },
    }
}

/// Widening (single to double) or narrowing (double to single) of `a` from `from` to `to`.
pub fn expect_convert(from: F, to: F, a: u64, fz: bool, dn: bool) -> Exp {
    let mut in_flags = 0;
    let a = flush(from, a, fz, &mut in_flags);
    let ex = |bits: u64, flags: u32| Exp::Exact {
        bits,
        flags: flags | in_flags,
    };
    match kind(from, a) {
        Kind::Nan { signaling } => {
            let flags = if signaling { IOC } else { 0 };
            if dn {
                ex(to.default_nan(), flags)
            } else {
                let frac = a & from.frac_mask();
                let frac = if to.fbits >= from.fbits {
                    frac << (to.fbits - from.fbits)
                } else {
                    frac >> (from.fbits - to.fbits)
                };
                let bits = (from.sign(a) << to.sign_shift()) | to.inf_mag() | frac | to.quiet_bit();
                ex(bits, flags)
            }
        }
        Kind::Inf(s) => ex(to.with_sign(to.inf_mag(), s), 0),
        Kind::Zero(s) => ex(to.with_sign(0, s), 0),
        Kind::Fin(s) => Exp::Round {
            t: Target::Ratio(lattice(from, from.mag(a))),
            sign: s,
            in_flags,
        },
    }
}

/// Integer to float: `x` is the integer value as (sign, magnitude).
pub fn expect_from_int(_f: F, sign: u64, mag: u128) -> Exp {
    if mag == 0 {
        return Exp::Exact { bits: 0, flags: 0 };
    }
    Exp::Round {
        t: Target::Ratio(Q::int(mag)),
        sign,
        in_flags: 0,
    }
}

/// Float to a 32-bit integer (signed or unsigned) by rounding mode `rm`, with the flags. The
/// magnitude is split at the binary point with shifts and masks.
pub fn expect_to_int(f: F, a: u64, rm: u32, fz: bool, signed: bool) -> (u32, u32) {
    let mut in_flags = 0;
    let a = flush(f, a, fz, &mut in_flags);
    let (lo, hi): (i128, i128) = if signed {
        (-(1 << 31), (1 << 31) - 1)
    } else {
        (0, (1 << 32) - 1)
    };
    let sat = |neg: bool| (if neg { lo } else { hi }) as i32 as u32;
    match kind(f, a) {
        Kind::Nan { .. } => (0, IOC),
        Kind::Inf(s) => (sat(s == 1), IOC | in_flags),
        Kind::Zero(_) => (0, in_flags),
        Kind::Fin(s) => {
            let e = (f.mag(a) >> f.fbits) as i64;
            let (m, exp) = if e == 0 {
                (u128::from(f.mag(a)), f.emin() - i64::from(f.fbits))
            } else {
                (
                    u128::from(f.mag(a) & f.frac_mask()) | (1u128 << f.fbits),
                    e - f.bias() - i64::from(f.fbits),
                )
            };
            // The integer part k, the round-up decision and whether the value is inexact.
            let (k, up, inex) = if exp >= 0 {
                if exp > 40 {
                    (1u128 << 100, false, false)
                } else {
                    (m << exp, false, false)
                }
            } else {
                let sh = (-exp) as u32;
                if sh > 100 {
                    // m < 2^53 is far below half a unit: the integer part is 0.
                    let up = (rm & 3 == 1 && s == 0) || (rm & 3 == 2 && s == 1);
                    (0, up, true)
                } else {
                    let k = m >> sh;
                    let frac = m & ((1u128 << sh) - 1);
                    let half = 1u128 << (sh - 1);
                    let up = match rm & 3 {
                        0 => frac > half || (frac == half && k & 1 == 1),
                        1 => frac != 0 && s == 0,
                        2 => frac != 0 && s == 1,
                        _ => false,
                    };
                    (k, up, frac != 0)
                }
            };
            let r = k + u128::from(up);
            let value: i128 = if s == 1 { -(r as i128) } else { r as i128 };
            if value < lo || value > hi {
                (sat(value < lo), IOC | in_flags)
            } else {
                let ixc = if inex { IXC } else { 0 };
                (value as i32 as u32, ixc | in_flags)
            }
        }
    }
}
