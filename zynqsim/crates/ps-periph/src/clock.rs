//! The clock rates the peripheral models count at, derived from the Cortex-A9 clock.
//!
//! The reference rates are the ones ps7_init leaves on the Zybo Z7-20 as remembered from
//! UG585 ch.25 (clocks): ARM PLL 1 GHz, so CPU 6x4x = 666.67 MHz, CPU 3x2x (PERIPHCLK) =
//! 333.33 MHz, CPU 1x (TTC) = 111.11 MHz. `VERIFY:` the exact divider values.

/// CPU clock (CPU_6OR4X), in hertz. Rounded from 1000 MHz x 2 / 3.
pub const CPU_HZ: u64 = 666_666_667;

/// PERIPHCLK (CPU_3OR2X) is the CPU clock divided by two. The private timer, the watchdog and
/// the global timer count at this rate.
pub const PERIPH_DIV: u64 = 2;

/// CPU_1X feeds the triple timer counters: the CPU clock divided by six.
pub const TTC_DIV: u64 = 6;

/// Default UART reference clock in hertz (`VERIFY:` UART_CLK_CTRL reset value and divider).
pub const UART_REF_HZ: u64 = 100_000_000;

/// Converts CPU cycles to whole ticks of a clock that runs at `CPU_HZ / div`, keeping the
/// remainder in `acc` so that no cycle is lost or counted twice.
pub fn ticks_of(acc: &mut u64, cycles: u64, div: u64) -> u64 {
    let total = *acc + cycles;
    *acc = total % div;
    total / div
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remainders_carry_between_calls() {
        let mut acc = 0;
        let mut ticks = 0;
        for _ in 0..7 {
            ticks += ticks_of(&mut acc, 1, PERIPH_DIV);
        }
        assert_eq!(ticks, 3);
        assert_eq!(acc, 1);
        assert_eq!(ticks_of(&mut acc, 1, PERIPH_DIV), 1);
    }
}
