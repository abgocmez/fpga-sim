//! Pure helpers shared by the executors: the A32 barrel shifter, add-with-carry and the
//! immediate expansion. Each returns the value and the carry-out the ARM ARM defines.

/// `AddWithCarry`: returns (sum, carry out, signed overflow).
pub(crate) fn add_with_carry(x: u32, y: u32, carry_in: bool) -> (u32, bool, bool) {
    let sum = u64::from(x) + u64::from(y) + u64::from(carry_in);
    let res = sum as u32;
    let carry = sum > u64::from(u32::MAX);
    let overflow = ((x ^ res) & (y ^ res) & 0x8000_0000) != 0;
    (res, carry, overflow)
}

/// Logical shift right by `n` (1..=32) with the carry-out of the last bit shifted out.
fn lsr(value: u32, n: u32) -> (u32, bool) {
    if n >= 32 {
        (0, value >> 31 != 0)
    } else {
        (value >> n, (value >> (n - 1)) & 1 != 0)
    }
}

/// Arithmetic shift right by `n` (1..=32).
fn asr(value: u32, n: u32) -> (u32, bool) {
    if n >= 32 {
        let sign = value >> 31 != 0;
        (if sign { u32::MAX } else { 0 }, sign)
    } else {
        (((value as i32) >> n) as u32, (value >> (n - 1)) & 1 != 0)
    }
}

/// Shift by an immediate (`imm5`): `ty` is the two-bit shift type (0 LSL, 1 LSR, 2 ASR,
/// 3 ROR / RRX). Returns (value, carry out).
pub(crate) fn shift_imm(value: u32, ty: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    match ty {
        0 => {
            if amount == 0 {
                (value, carry_in)
            } else {
                (value << amount, (value >> (32 - amount)) & 1 != 0)
            }
        }
        1 => lsr(value, if amount == 0 { 32 } else { amount }),
        2 => asr(value, if amount == 0 { 32 } else { amount }),
        _ => {
            if amount == 0 {
                // RRX: rotate right by one through the carry flag.
                ((u32::from(carry_in) << 31) | (value >> 1), value & 1 != 0)
            } else {
                let r = value.rotate_right(amount);
                (r, r >> 31 != 0)
            }
        }
    }
}

/// Shift by the low byte of a register. An amount of zero leaves the value and carry alone.
pub(crate) fn shift_reg(value: u32, ty: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        return (value, carry_in);
    }
    match ty {
        0 => match amount {
            1..=31 => (value << amount, (value >> (32 - amount)) & 1 != 0),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        1 => {
            if amount <= 32 {
                lsr(value, amount)
            } else {
                (0, false)
            }
        }
        2 => asr(value, amount.min(32)),
        _ => {
            let m = amount & 31;
            if m == 0 {
                (value, value >> 31 != 0)
            } else {
                let r = value.rotate_right(m);
                (r, r >> 31 != 0)
            }
        }
    }
}

/// The 12-bit immediate of a data-processing instruction: an 8-bit value rotated right by
/// twice the 4-bit field. With no rotation the carry is the incoming C flag.
pub(crate) fn expand_imm(insn: u32, carry_in: bool) -> (u32, bool) {
    let rot = ((insn >> 8) & 0xF) * 2;
    let v = (insn & 0xFF).rotate_right(rot);
    if rot == 0 {
        (v, carry_in)
    } else {
        (v, v >> 31 != 0)
    }
}
