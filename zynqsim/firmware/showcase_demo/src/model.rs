//! Pure model of the showcase demo: the input signal, the two coefficient sets and the
//! software reference FIR. No hardware access and no `rt` dependency, so the host unit tests
//! in `host-test/` include this file unchanged (`#[path]`).

/// Samples per pass.
pub const N: usize = 64;
/// FIR taps.
pub const TAPS: usize = 8;

/// Coefficient set A: lowpass.
pub const COEF_LOW: [i16; TAPS] = [1, 2, 3, 4, 4, 3, 2, 1];
/// Coefficient set B: highpass (alternating signs).
pub const COEF_HIGH: [i16; TAPS] = [1, -1, 1, -1, 1, -1, 1, -1];

/// Linear congruential generator (the classic glibc constants), 32-bit state.
pub struct Lcg(u32);

impl Lcg {
    pub const fn new(seed: u32) -> Self {
        Lcg(seed)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        self.0
    }
}

/// The input signal: a square wave of +-1000 (half period 8 samples) plus noise in -32..=32
/// from an LCG with seed 1.
// ASSUMPTION: the spec only says "square wave of amplitude +-1000 with a small deterministic
// noise (fixed LCG, seed 1)"; the half period (8 samples), the noise range (+-32, taken from
// bits 31..16 modulo 65) and using the first LCG output for sample 0 are chosen here.
pub fn input_signal() -> [i16; N] {
    let mut lcg = Lcg::new(1);
    let mut out = [0i16; N];
    for (i, s) in out.iter_mut().enumerate() {
        let base: i32 = if (i / 8) % 2 == 0 { 1000 } else { -1000 };
        let noise = ((lcg.next_u32() >> 16) as i32) % 65 - 32;
        *s = (base + noise) as i16;
    }
    out
}

/// Software reference: y[n] = sum_{k=0..7} coef[k] * x[n-k], with x[m] = 0 for m < 0.
pub fn fir_ref(coef: &[i16; TAPS], x: &[i16; N]) -> [i32; N] {
    let mut y = [0i32; N];
    for (n, out) in y.iter_mut().enumerate() {
        *out = coef
            .iter()
            .enumerate()
            .take(n + 1)
            .map(|(k, &c)| i32::from(c) * i32::from(x[n - k]))
            .sum();
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcg_first_output() {
        let mut l = Lcg::new(1);
        assert_eq!(l.next_u32(), 1_103_527_590);
    }

    #[test]
    fn signal_first_sample_is_hand_checked() {
        // 1103527590 >> 16 = 16838; 16838 % 65 = 3; 3 - 32 = -29; 1000 - 29 = 971.
        let x = input_signal();
        assert_eq!(x[0], 971);
    }

    #[test]
    fn signal_square_wave_with_bounded_noise() {
        let x = input_signal();
        for (i, &s) in x.iter().enumerate() {
            let base = if (i / 8) % 2 == 0 { 1000 } else { -1000 };
            assert!((i32::from(s) - base).abs() <= 32, "sample {i} = {s}");
        }
    }

    #[test]
    fn signal_is_deterministic() {
        assert_eq!(input_signal(), input_signal());
    }

    #[test]
    fn impulse_gives_the_coefficients() {
        let mut x = [0i16; N];
        x[0] = 1;
        let y = fir_ref(&COEF_LOW, &x);
        assert_eq!(&y[..9], &[1, 2, 3, 4, 4, 3, 2, 1, 0]);
        assert!(y[9..].iter().all(|&v| v == 0));
    }

    #[test]
    fn step_gives_running_sums() {
        let x = [1i16; N];
        let y = fir_ref(&COEF_LOW, &x);
        // Running sums of 1,2,3,4,4,3,2,1 saturate at 20 after eight samples.
        assert_eq!(&y[..9], &[1, 3, 6, 10, 14, 17, 19, 20, 20]);
    }

    #[test]
    fn highpass_on_constant_input() {
        let x = [1i16; N];
        let y = fir_ref(&COEF_HIGH, &x);
        // n = 0..=6: sum of (-1)^k over k = 0..=n is 1 for even n, 0 for odd n.
        assert_eq!(&y[..7], &[1, 0, 1, 0, 1, 0, 1]);
        // n >= 7: all eight taps are inside the window, the sum is 0.
        assert_eq!(&y[7..12], &[0, 0, 0, 0, 0]);
    }

    #[test]
    fn negative_sample_through_highpass() {
        let mut x = [0i16; N];
        x[2] = -3;
        let y = fir_ref(&COEF_HIGH, &x);
        // y[2] = c[0]*x[2] = -3; y[3] = c[1]*x[2] = +3; y[4] = c[2]*x[2] = -3.
        assert_eq!(&y[2..5], &[-3, 3, -3]);
    }
}
