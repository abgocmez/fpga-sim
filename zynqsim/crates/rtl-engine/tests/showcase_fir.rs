//! The showcase's `zs_fir` (fixtures/showcase/rtl) behind an AXI4-Lite slave, driven from Rust
//! through `RtlModule`: the register map, the FIR values against hand-computed ones and against
//! an i32 reference, the interrupt level and the "ignored while busy / disabled" rules.

use pl_native::RegisterBlock;
use rtl_engine::{Design, RtlModule};
use std::path::PathBuf;

const CTRL: u64 = 0x00;
const STATUS: u64 = 0x04;
const COUNT: u64 = 0x08;
const ID: u64 = 0x0C;
const COEF0: u64 = 0x10;
const DIN: u64 = 0x40;
const DOUT: u64 = 0x44;

fn fir() -> RtlModule {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("showcase")
        .join("rtl")
        .join("zs_fir.v");
    let design = Design::elaborate("zs_fir", &[path], &[], &[]);
    assert!(
        design.diagnostics.is_empty(),
        "zs_fir must elaborate clean: {:?}",
        design.diagnostics
    );
    let mut m = RtlModule::new(design).expect("AXI4-Lite slave detected");
    m.set_irq_output("irq").unwrap();
    m
}

fn set_coefs(m: &mut RtlModule, c: [i16; 8]) {
    for (k, v) in c.iter().enumerate() {
        m.write(COEF0 + 4 * k as u64, *v as u16 as u32, 0xF).unwrap();
    }
}

/// Pushes `x`, waits for DONE (polling STATUS), acks it and returns DOUT.
fn push(m: &mut RtlModule, x: i16) -> i32 {
    m.write(DIN, x as u16 as u32, 0xF).unwrap();
    for _ in 0..100 {
        if m.read(STATUS).unwrap() & 2 != 0 {
            let y = m.read(DOUT).unwrap() as i32;
            m.write(STATUS, 2, 0xF).unwrap();
            return y;
        }
    }
    panic!("DONE never set");
}

fn reference(coef: &[i16; 8], xs: &[i16]) -> Vec<i32> {
    (0..xs.len())
        .map(|n| {
            (0..8)
                .map(|k| {
                    if n >= k {
                        i32::from(coef[k]).wrapping_mul(i32::from(xs[n - k]))
                    } else {
                        0
                    }
                })
                .fold(0i32, i32::wrapping_add)
        })
        .collect()
}

#[test]
fn id_and_reset_state() {
    let mut m = fir();
    assert_eq!(m.read(ID).unwrap(), 0x5A46_4952);
    assert_eq!(m.read(CTRL).unwrap(), 0);
    assert_eq!(m.read(STATUS).unwrap(), 0);
    assert_eq!(m.read(COUNT).unwrap(), 0);
    assert_eq!(m.read(DOUT).unwrap(), 0);
    assert_eq!(m.read(0x3C).unwrap(), 0, "unlisted offset reads 0");
    assert_eq!(m.read(0x48).unwrap(), 0, "unlisted offset reads 0");
}

#[test]
fn coefficients_read_back_sign_extended() {
    let mut m = fir();
    set_coefs(&mut m, [1, -1, 32767, -32768, 0, -2, 3, -4]);
    let got: Vec<i32> = (0..8)
        .map(|k| m.read(COEF0 + 4 * k).unwrap() as i32)
        .collect();
    assert_eq!(got, vec![1, -1, 32767, -32768, 0, -2, 3, -4]);
}

#[test]
fn the_impulse_response_is_the_coefficient_set() {
    // Hand-computed: x = 1, 0, 0, ... gives y[n] = COEF[n], then zeros.
    let mut m = fir();
    set_coefs(&mut m, [1, 2, 3, 4, 4, 3, 2, 1]);
    m.write(CTRL, 1, 0xF).unwrap();
    let mut y = vec![push(&mut m, 1)];
    for _ in 0..9 {
        y.push(push(&mut m, 0));
    }
    assert_eq!(y, vec![1, 2, 3, 4, 4, 3, 2, 1, 0, 0]);
    assert_eq!(m.read(COUNT).unwrap(), 10);
}

#[test]
fn a_few_samples_by_hand() {
    // coef = [1,2,3,4,4,3,2,1]; x = 100, -50, 7
    // y0 = 1*100                       = 100
    // y1 = 1*-50 + 2*100               = 150
    // y2 = 1*7 + 2*-50 + 3*100         = 207
    let mut m = fir();
    set_coefs(&mut m, [1, 2, 3, 4, 4, 3, 2, 1]);
    m.write(CTRL, 1, 0xF).unwrap();
    assert_eq!(push(&mut m, 100), 100);
    assert_eq!(push(&mut m, -50), 150);
    assert_eq!(push(&mut m, 7), 207);
}

#[test]
fn matches_the_reference_for_both_showcase_filters_and_extremes() {
    let low = [1, 2, 3, 4, 4, 3, 2, 1];
    let high = [1, -1, 1, -1, 1, -1, 1, -1];
    let extreme = [32767, -32768, 32767, -32768, -1, 1, -32768, 32767];
    let xs: Vec<i16> = (0..40)
        .map(|n| match n % 5 {
            0 => 1000,
            1 => -1000,
            2 => 32767,
            3 => -32768,
            _ => (n * 37 - 300) as i16,
        })
        .collect();
    for coef in [low, high, extreme] {
        let mut m = fir();
        set_coefs(&mut m, coef);
        m.write(CTRL, 1, 0xF).unwrap();
        let want = reference(&coef, &xs);
        let got: Vec<i32> = xs.iter().map(|&x| push(&mut m, x)).collect();
        assert_eq!(got, want, "coefficients {coef:?}");
    }
}

#[test]
fn clear_empties_the_delay_line_and_the_count() {
    let mut m = fir();
    set_coefs(&mut m, [1, 2, 3, 4, 4, 3, 2, 1]);
    m.write(CTRL, 1, 0xF).unwrap();
    push(&mut m, 500);
    push(&mut m, 500);
    assert_eq!(m.read(COUNT).unwrap(), 2);
    m.write(CTRL, 1 | 4, 0xF).unwrap();
    assert_eq!(m.read(CTRL).unwrap(), 1, "CLEAR reads 0");
    assert_eq!(m.read(COUNT).unwrap(), 0);
    assert_eq!(m.read(STATUS).unwrap(), 0);
    assert_eq!(push(&mut m, 10), 10, "x[m<0] = 0 after CLEAR");
}

#[test]
fn din_is_ignored_when_disabled_or_busy() {
    let mut m = fir();
    set_coefs(&mut m, [1, 2, 3, 4, 4, 3, 2, 1]);
    // disabled
    m.write(DIN, 99, 0xF).unwrap();
    assert_eq!(m.read(COUNT).unwrap(), 0);
    assert_eq!(m.read(STATUS).unwrap(), 0);
    // busy: the second write comes while the MAC runs
    m.write(CTRL, 1, 0xF).unwrap();
    m.write(DIN, 10, 0xF).unwrap();
    let st = m.read(STATUS).unwrap();
    assert_eq!(st & 1, 1, "BUSY right after DIN");
    m.write(DIN, 77, 0xF).unwrap();
    for _ in 0..100 {
        if m.read(STATUS).unwrap() & 2 != 0 {
            break;
        }
    }
    assert_eq!(m.read(COUNT).unwrap(), 1, "the DIN while BUSY is ignored");
    assert_eq!(m.read(DOUT).unwrap(), 10);
    assert_eq!(m.read(STATUS).unwrap(), 2, "DONE, not BUSY");
}

#[test]
fn irq_is_done_and_irq_en_and_done_is_write_one_to_clear() {
    let mut m = fir();
    set_coefs(&mut m, [1, 0, 0, 0, 0, 0, 0, 0]);
    m.write(CTRL, 1 | 2, 0xF).unwrap();
    assert!(!m.irq_level());
    m.write(DIN, 5, 0xF).unwrap();
    for _ in 0..100 {
        if m.irq_level() {
            break;
        }
        m.read(STATUS).unwrap();
    }
    assert!(m.irq_level(), "irq rises with DONE");
    assert_eq!(m.read(DOUT).unwrap(), 5);
    m.write(STATUS, 1, 0xF).unwrap();
    assert!(m.irq_level(), "writing bit0 does not clear DONE");
    m.write(STATUS, 2, 0xF).unwrap();
    assert!(!m.irq_level(), "W1C on bit1 drops the level");
    assert_eq!(m.read(STATUS).unwrap(), 0);
    // IRQ_EN off: DONE alone raises nothing
    m.write(CTRL, 1, 0xF).unwrap();
    assert_eq!(push(&mut m, 6), 6);
    m.write(DIN, 7, 0xF).unwrap();
    for _ in 0..30 {
        m.read(STATUS).unwrap();
    }
    assert!(!m.irq_level());
}
