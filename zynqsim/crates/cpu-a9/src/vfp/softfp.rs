//! Integer-only IEEE 754 binary32 and binary64 arithmetic with the rules the VFP's FPSCR
//! selects: rounding mode (RMode), flush-to-zero (FZ), default NaN (DN) and the cumulative
//! exception bits. No host floating-point operation is used: every result comes from 128-bit
//! integer arithmetic and one rounding routine, [`pack`].
//!
//! Conventions, chosen where the ARM ARM leaves room (recorded in ADR-030):
//! * Tininess is detected before rounding: an inexact result below 2^emin sets UFC.
//! * FZ: a subnormal input is a signed zero (IDC is raised); a result that rounds to zero or to
//!   a subnormal becomes a signed zero with UFC and IXC.
//! * NaN operands: the first signaling NaN in operand order is quietened and returned and IOC is
//!   raised; otherwise the first quiet NaN is returned. With DN the default NaN is returned.
//! * An invalid operation with no NaN operand returns the default NaN (positive, quiet).
//! * Overflow sets OFC and IXC; the result is infinity or the largest finite value by the mode.

/// FPSCR.IOC: invalid operation.
pub(crate) const IOC: u32 = 1 << 0;
/// FPSCR.DZC: division by zero.
pub(crate) const DZC: u32 = 1 << 1;
/// FPSCR.OFC: overflow.
pub(crate) const OFC: u32 = 1 << 2;
/// FPSCR.UFC: underflow.
pub(crate) const UFC: u32 = 1 << 3;
/// FPSCR.IXC: inexact.
pub(crate) const IXC: u32 = 1 << 4;
/// FPSCR.IDC: input denormal flushed by FZ.
pub(crate) const IDC: u32 = 1 << 7;

/// The four IEEE rounding directions, as FPSCR.RMode encodes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Round {
    /// Round to nearest, ties to even (RMode 0b00).
    Nearest,
    /// Toward +infinity (0b01).
    Up,
    /// Toward -infinity (0b10).
    Down,
    /// Toward zero (0b11).
    Zero,
}

impl Round {
    /// The mode of an FPSCR.RMode field (the low two bits of `bits` are used).
    pub(crate) fn from_bits(bits: u32) -> Round {
        match bits & 3 {
            0 => Round::Nearest,
            1 => Round::Up,
            2 => Round::Down,
            _ => Round::Zero,
        }
    }
}

/// The FPSCR settings an operation depends on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Env {
    pub(crate) rm: Round,
    pub(crate) fz: bool,
    pub(crate) dn: bool,
}

/// An IEEE binary format: exponent and fraction widths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fmt {
    pub(crate) ebits: u32,
    pub(crate) fbits: u32,
}

/// binary32.
pub(crate) const SINGLE: Fmt = Fmt {
    ebits: 8,
    fbits: 23,
};
/// binary64.
pub(crate) const DOUBLE: Fmt = Fmt {
    ebits: 11,
    fbits: 52,
};

impl Fmt {
    fn sign_shift(self) -> u32 {
        self.ebits + self.fbits
    }
    fn exp_max(self) -> u64 {
        (1u64 << self.ebits) - 1
    }
    fn frac_mask(self) -> u64 {
        (1u64 << self.fbits) - 1
    }
    fn bias(self) -> i64 {
        (1i64 << (self.ebits - 1)) - 1
    }
    fn emin(self) -> i64 {
        1 - self.bias()
    }
    fn emin_lsb(self) -> i64 {
        self.emin() - i64::from(self.fbits)
    }

    /// The sign bit of `v` (0 or 1).
    pub(crate) fn sign(self, v: u64) -> u64 {
        (v >> self.sign_shift()) & 1
    }
    /// `v` with its sign bit flipped (also for NaN: a plain sign operation).
    pub(crate) fn negate(self, v: u64) -> u64 {
        v ^ (1u64 << self.sign_shift())
    }
    /// `v` with its sign bit cleared.
    pub(crate) fn abs(self, v: u64) -> u64 {
        v & !(1u64 << self.sign_shift())
    }
    /// A signed zero.
    pub(crate) fn zero(self, sign: u64) -> u64 {
        sign << self.sign_shift()
    }
    /// A signed infinity.
    pub(crate) fn inf(self, sign: u64) -> u64 {
        self.zero(sign) | (self.exp_max() << self.fbits)
    }
    /// The largest finite value with the given sign.
    pub(crate) fn max_finite(self, sign: u64) -> u64 {
        self.zero(sign) | ((self.exp_max() - 1) << self.fbits) | self.frac_mask()
    }
    /// The default NaN: positive, quiet, zero payload.
    pub(crate) fn default_nan(self) -> u64 {
        (self.exp_max() << self.fbits) | (1u64 << (self.fbits - 1))
    }
    fn quiet(self, v: u64) -> u64 {
        v | (1u64 << (self.fbits - 1))
    }
}

/// A classified operand. `Fin` is `sig * 2^exp` with `sig` in `[2^fbits, 2^(fbits+1))`, so
/// subnormal inputs are normalised here.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Val {
    Nan { signaling: bool },
    Inf { sign: u64 },
    Zero { sign: u64 },
    Fin { sign: u64, exp: i64, sig: u128 },
}

/// Classifies `v`. With FZ a subnormal is a signed zero and raises IDC.
pub(crate) fn unpack(f: Fmt, v: u64, env: &Env, fl: &mut u32) -> Val {
    let sign = f.sign(v);
    let e = (v >> f.fbits) & f.exp_max();
    let frac = v & f.frac_mask();
    if e == f.exp_max() {
        return if frac == 0 {
            Val::Inf { sign }
        } else {
            Val::Nan {
                signaling: frac & (1u64 << (f.fbits - 1)) == 0,
            }
        };
    }
    if e == 0 {
        if frac == 0 {
            return Val::Zero { sign };
        }
        if env.fz {
            *fl |= IDC;
            return Val::Zero { sign };
        }
        let top = 63 - frac.leading_zeros();
        let shift = f.fbits - top;
        return Val::Fin {
            sign,
            exp: f.emin() - i64::from(f.fbits) - i64::from(shift),
            sig: u128::from(frac << shift),
        };
    }
    Val::Fin {
        sign,
        exp: e as i64 - f.bias() - i64::from(f.fbits),
        sig: u128::from(frac | (1u64 << f.fbits)),
    }
}

/// Whether the magnitude is rounded up, given the mode, the sign, the last kept bit, the
/// rounding bit and the sticky (any lower nonzero bit) of the discarded part.
fn round_up(rm: Round, sign: u64, q_odd: bool, rbit: bool, rest: bool) -> bool {
    let inexact = rbit || rest;
    match rm {
        Round::Nearest => rbit && (rest || q_odd),
        Round::Up => inexact && sign == 0,
        Round::Down => inexact && sign == 1,
        Round::Zero => false,
    }
}

/// Splits `sig` at `drop` low bits: the kept part, the rounding bit and whether anything
/// nonzero lies below it (including `sticky`). `drop <= 0` shifts left (exact, `sticky` must
/// be false then).
fn split(sig: u128, drop: i64, sticky: bool) -> (u128, bool, bool) {
    if drop <= 0 {
        return (sig << ((-drop) as u32), false, sticky);
    }
    if drop >= 128 {
        return (0, false, sig != 0 || sticky);
    }
    let d = drop as u32;
    let q = sig >> d;
    let rbit = (sig >> (d - 1)) & 1 == 1;
    let rest = sig & ((1u128 << (d - 1)) - 1) != 0 || sticky;
    (q, rbit, rest)
}

/// Rounds `(sig + fraction) * 2^exp` to format `f` and packs it. `sig` is nonzero; `sticky`
/// says the exact value has a nonzero part below the LSB of `sig`. Callers keep at least two
/// bits of `sig` above the format precision whenever `sticky` is set.
fn pack(f: Fmt, sign: u64, exp: i64, sig: u128, sticky: bool, env: &Env, fl: &mut u32) -> u64 {
    let p = f.fbits + 1;
    let e_top = exp + i64::from(127 - sig.leading_zeros());
    let tiny = e_top < f.emin();
    let lsb = (e_top - i64::from(p - 1)).max(f.emin_lsb());
    let (q, rbit, rest) = split(sig, lsb - exp, sticky);
    let inexact = rbit || rest;
    let mut q = q + u128::from(round_up(env.rm, sign, q & 1 == 1, rbit, rest));
    let mut lsb = lsb;
    if q == 1u128 << p {
        q >>= 1;
        lsb += 1;
    }
    if inexact {
        *fl |= IXC;
        if tiny {
            *fl |= UFC;
        }
    }
    if q < 1u128 << f.fbits {
        // Zero or subnormal after rounding.
        if env.fz {
            *fl |= UFC | IXC;
            return f.zero(sign);
        }
        return f.zero(sign) | (q as u64);
    }
    let e = lsb + i64::from(p - 1);
    if e > f.bias() {
        *fl |= OFC | IXC;
        let to_max = match (env.rm, sign) {
            (Round::Nearest, _) => false,
            (Round::Zero, _) => true,
            (Round::Up, s) => s == 1,
            (Round::Down, s) => s == 0,
        };
        return if to_max {
            f.max_finite(sign)
        } else {
            f.inf(sign)
        };
    }
    f.zero(sign) | (((e + f.bias()) as u64) << f.fbits) | (q as u64 & f.frac_mask())
}

/// NaN result of an operation with NaN operands (in operand order).
fn propagate(f: Fmt, env: &Env, fl: &mut u32, ops: &[(u64, Val)]) -> u64 {
    let signaling = ops
        .iter()
        .find(|(_, v)| matches!(v, Val::Nan { signaling: true }));
    let any = ops.iter().find(|(_, v)| matches!(v, Val::Nan { .. }));
    if signaling.is_some() {
        *fl |= IOC;
    }
    match signaling.or(any) {
        Some((bits, _)) if !env.dn => f.quiet(*bits),
        _ => f.default_nan(),
    }
}

/// The invalid-operation result with no NaN operand.
fn invalid(f: Fmt, fl: &mut u32) -> u64 {
    *fl |= IOC;
    f.default_nan()
}

fn negate_val(v: Val) -> Val {
    match v {
        Val::Inf { sign } => Val::Inf { sign: sign ^ 1 },
        Val::Zero { sign } => Val::Zero { sign: sign ^ 1 },
        Val::Fin { sign, exp, sig } => Val::Fin {
            sign: sign ^ 1,
            exp,
            sig,
        },
        Val::Nan { .. } => v,
    }
}

/// Exact sum of two finite nonzero values (`hi` has the larger exponent).
fn add_finite(f: Fmt, a: (u64, i64, u128), b: (u64, i64, u128), env: &Env, fl: &mut u32) -> u64 {
    let (sa, ea, ma) = a;
    let (sb, eb, mb) = b;
    let (hs, eh, mh, ls, el, ml) = if ea >= eb {
        (sa, ea, ma, sb, eb, mb)
    } else {
        (sb, eb, mb, sa, ea, ma)
    };
    let d = eh - el;
    // The larger operand scaled by 2^64; the smaller one aligned into the same grid. Bits
    // shifted out below the grid are kept as a sticky flag.
    let big = mh << 64;
    let (small, sticky) = if d <= 64 {
        (ml << ((64 - d) as u32), false)
    } else {
        let sh = (d - 64) as u32;
        if sh >= 128 {
            (0, true)
        } else {
            (ml >> sh, ml & ((1u128 << sh) - 1) != 0)
        }
    };
    let exp = eh - 64;
    if hs == ls {
        return pack(f, hs, exp, big + small, sticky, env, fl);
    }
    let (sign, diff) = if sticky {
        // big > small by at least 2^116 here, so big - small - 1 is positive.
        (hs, big - small - 1)
    } else if big >= small {
        (hs, big - small)
    } else {
        (ls, small - big)
    };
    if diff == 0 && !sticky {
        // Exact cancellation: +0, or -0 when rounding toward -infinity.
        return f.zero(u64::from(env.rm == Round::Down));
    }
    pack(f, sign, exp, diff, sticky, env, fl)
}

/// `a + b`, or `a - b` when `subtract` (the sign of `b` is flipped, NaN payloads are not).
pub(crate) fn add(f: Fmt, a: u64, b: u64, subtract: bool, env: &Env, fl: &mut u32) -> u64 {
    let va = unpack(f, a, env, fl);
    let vb0 = unpack(f, b, env, fl);
    if matches!(va, Val::Nan { .. }) || matches!(vb0, Val::Nan { .. }) {
        return propagate(f, env, fl, &[(a, va), (b, vb0)]);
    }
    let vb = if subtract { negate_val(vb0) } else { vb0 };
    match (va, vb) {
        (Val::Inf { sign: sa }, Val::Inf { sign: sb }) => {
            if sa == sb {
                f.inf(sa)
            } else {
                invalid(f, fl)
            }
        }
        (Val::Inf { sign }, _) | (_, Val::Inf { sign }) => f.inf(sign),
        (Val::Zero { sign: sa }, Val::Zero { sign: sb }) => f.zero(if sa == sb {
            sa
        } else {
            u64::from(env.rm == Round::Down)
        }),
        (Val::Zero { .. }, Val::Fin { sign, exp, sig })
        | (Val::Fin { sign, exp, sig }, Val::Zero { .. }) => {
            pack(f, sign, exp, sig, false, env, fl)
        }
        (
            Val::Fin {
                sign: sa,
                exp: ea,
                sig: ma,
            },
            Val::Fin {
                sign: sb,
                exp: eb,
                sig: mb,
            },
        ) => add_finite(f, (sa, ea, ma), (sb, eb, mb), env, fl),
        // NaN operands returned above.
        _ => f.default_nan(),
    }
}

/// `a * b`.
pub(crate) fn mul(f: Fmt, a: u64, b: u64, env: &Env, fl: &mut u32) -> u64 {
    let va = unpack(f, a, env, fl);
    let vb = unpack(f, b, env, fl);
    if matches!(va, Val::Nan { .. }) || matches!(vb, Val::Nan { .. }) {
        return propagate(f, env, fl, &[(a, va), (b, vb)]);
    }
    match (va, vb) {
        (Val::Inf { sign: sa }, Val::Inf { sign: sb })
        | (Val::Inf { sign: sa }, Val::Fin { sign: sb, .. })
        | (Val::Fin { sign: sa, .. }, Val::Inf { sign: sb }) => f.inf(sa ^ sb),
        (Val::Inf { .. }, Val::Zero { .. }) | (Val::Zero { .. }, Val::Inf { .. }) => invalid(f, fl),
        (Val::Zero { sign: sa }, Val::Zero { sign: sb })
        | (Val::Zero { sign: sa }, Val::Fin { sign: sb, .. })
        | (Val::Fin { sign: sa, .. }, Val::Zero { sign: sb }) => f.zero(sa ^ sb),
        (
            Val::Fin {
                sign: sa,
                exp: ea,
                sig: ma,
            },
            Val::Fin {
                sign: sb,
                exp: eb,
                sig: mb,
            },
        ) => pack(f, sa ^ sb, ea + eb, ma * mb, false, env, fl),
        _ => f.default_nan(),
    }
}

/// `a / b`.
pub(crate) fn div(f: Fmt, a: u64, b: u64, env: &Env, fl: &mut u32) -> u64 {
    let va = unpack(f, a, env, fl);
    let vb = unpack(f, b, env, fl);
    if matches!(va, Val::Nan { .. }) || matches!(vb, Val::Nan { .. }) {
        return propagate(f, env, fl, &[(a, va), (b, vb)]);
    }
    match (va, vb) {
        (Val::Inf { .. }, Val::Inf { .. }) | (Val::Zero { .. }, Val::Zero { .. }) => invalid(f, fl),
        (Val::Inf { sign: sa }, Val::Fin { sign: sb, .. })
        | (Val::Inf { sign: sa }, Val::Zero { sign: sb }) => f.inf(sa ^ sb),
        (Val::Fin { sign: sa, .. }, Val::Inf { sign: sb })
        | (Val::Zero { sign: sa }, Val::Inf { sign: sb })
        | (Val::Zero { sign: sa }, Val::Fin { sign: sb, .. }) => f.zero(sa ^ sb),
        (Val::Fin { sign: sa, .. }, Val::Zero { sign: sb }) => {
            *fl |= DZC;
            f.inf(sa ^ sb)
        }
        (
            Val::Fin {
                sign: sa,
                exp: ea,
                sig: ma,
            },
            Val::Fin {
                sign: sb,
                exp: eb,
                sig: mb,
            },
        ) => {
            // The quotient has at least 69 significant bits, far more than the precision.
            let num = ma << 70;
            let q = num / mb;
            let sticky = num % mb != 0;
            pack(f, sa ^ sb, ea - eb - 70, q, sticky, env, fl)
        }
        _ => f.default_nan(),
    }
}

/// Integer square root (floor).
fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = 1u128 << (128 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// `sqrt(a)`.
pub(crate) fn sqrt(f: Fmt, a: u64, env: &Env, fl: &mut u32) -> u64 {
    let va = unpack(f, a, env, fl);
    match va {
        Val::Nan { .. } => propagate(f, env, fl, &[(a, va)]),
        Val::Inf { sign: 0 } => f.inf(0),
        Val::Inf { .. } => invalid(f, fl),
        Val::Zero { sign } => f.zero(sign),
        Val::Fin { sign: 1, .. } => invalid(f, fl),
        Val::Fin { exp, sig, .. } => {
            let (e, m) = if exp % 2 != 0 {
                (exp - 1, sig << 1)
            } else {
                (exp, sig)
            };
            // sqrt(m * 2^(e+70)) with an even exponent: integer root of m << 70.
            let r = m << 70;
            let root = isqrt(r);
            let inexact = root * root != r;
            pack(f, 0, (e - 70) / 2, root, inexact, env, fl)
        }
    }
}

/// Converts between the two formats (exact widening; rounding when narrowing).
pub(crate) fn convert(from: Fmt, to: Fmt, a: u64, env: &Env, fl: &mut u32) -> u64 {
    match unpack(from, a, env, fl) {
        Val::Nan { signaling } => {
            if signaling {
                *fl |= IOC;
            }
            if env.dn {
                return to.default_nan();
            }
            let frac = a & from.frac_mask();
            let frac = if to.fbits >= from.fbits {
                frac << (to.fbits - from.fbits)
            } else {
                frac >> (from.fbits - to.fbits)
            };
            to.zero(from.sign(a)) | (to.exp_max() << to.fbits) | frac | (1u64 << (to.fbits - 1))
        }
        Val::Inf { sign } => to.inf(sign),
        Val::Zero { sign } => to.zero(sign),
        Val::Fin { sign, exp, sig } => pack(to, sign, exp, sig, false, env, fl),
    }
}

/// The value `(-1)^sign * mag * 2^scale` rounded to format `f`; an integer zero is +0.
pub(crate) fn from_int(f: Fmt, sign: u64, mag: u64, scale: i64, env: &Env, fl: &mut u32) -> u64 {
    if mag == 0 {
        return f.zero(0);
    }
    pack(f, sign, scale, u128::from(mag), false, env, fl)
}

/// Rounds `a * 2^scale` to an integer of `width` bits (signed or unsigned) with mode `rm`, and
/// returns it as two's complement in a `u32` (sign-extended for narrow signed results).
/// Out-of-range values, infinities and NaNs saturate or become 0 and raise IOC.
#[allow(clippy::too_many_arguments)] // the FPSCR-derived settings and the operand format are separate on purpose
pub(crate) fn to_int(
    f: Fmt,
    a: u64,
    scale: i64,
    rm: Round,
    signed: bool,
    width: u32,
    env: &Env,
    fl: &mut u32,
) -> u32 {
    let (lo, hi): (i128, i128) = if signed {
        (-(1i128 << (width - 1)), (1i128 << (width - 1)) - 1)
    } else {
        (0, (1i128 << width) - 1)
    };
    let saturate = |v: i128| (if v < lo { lo } else { hi }) as i32 as u32;
    let (sign, mag, inexact) = match unpack(f, a, env, fl) {
        Val::Nan { .. } => {
            *fl |= IOC;
            return 0;
        }
        Val::Inf { sign } => {
            *fl |= IOC;
            let v = if sign == 1 { lo } else { hi };
            return v as i32 as u32;
        }
        Val::Zero { .. } => return 0,
        Val::Fin { sign, exp, sig } => {
            let e = exp + scale;
            if e >= 0 {
                if e > 64 {
                    (sign, 1u128 << 100, false)
                } else {
                    (sign, sig << (e as u32), false)
                }
            } else {
                let (q, rbit, rest) = split(sig, -e, false);
                let up = round_up(rm, sign, q & 1 == 1, rbit, rest);
                (sign, q + u128::from(up), rbit || rest)
            }
        }
    };
    let value: i128 = if sign == 1 {
        -(mag as i128)
    } else {
        mag as i128
    };
    if value < lo || value > hi {
        *fl |= IOC;
        return saturate(value);
    }
    if inexact {
        *fl |= IXC;
    }
    value as i32 as u32
}

/// Orders the bit patterns as numbers (zeros equal, infinities at the ends).
fn key(f: Fmt, bits: u64, v: &Val) -> i128 {
    let mag = match v {
        Val::Zero { .. } => 0,
        _ => i128::from(f.abs(bits)),
    };
    if f.sign(bits) == 1 { -mag } else { mag }
}

/// VCMP / VCMPE: the NZCV nibble (in bits 31:28) for `a` compared with `b`. `signaling` is
/// VCMPE: any NaN raises IOC; VCMP raises it only for a signaling NaN.
pub(crate) fn compare(f: Fmt, a: u64, b: u64, signaling: bool, env: &Env, fl: &mut u32) -> u32 {
    let va = unpack(f, a, env, fl);
    let vb = unpack(f, b, env, fl);
    if matches!(va, Val::Nan { .. }) || matches!(vb, Val::Nan { .. }) {
        let any_signaling = matches!(va, Val::Nan { signaling: true })
            || matches!(vb, Val::Nan { signaling: true });
        if signaling || any_signaling {
            *fl |= IOC;
        }
        return 0b0011 << 28;
    }
    let (ka, kb) = (key(f, a, &va), key(f, b, &vb));
    if ka == kb {
        0b0110 << 28
    } else if ka < kb {
        0b1000 << 28
    } else {
        0b0010 << 28
    }
}

/// VFPExpandImm: the 8-bit immediate of VMOV as a value of format `f`.
pub(crate) fn expand_imm(f: Fmt, imm8: u32) -> u64 {
    let a = u64::from((imm8 >> 7) & 1);
    let b = u64::from((imm8 >> 6) & 1);
    let c = u64::from((imm8 >> 4) & 3);
    let d = u64::from(imm8 & 0xF);
    let exp = ((1 - b) << (f.ebits - 1)) | ((b * ((1u64 << (f.ebits - 3)) - 1)) << 2) | c;
    (a << f.sign_shift()) | (exp << f.fbits) | (d << (f.fbits - 4))
}
