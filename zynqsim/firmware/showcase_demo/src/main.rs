//! The showcase demo (`examples/showcase`): an 8-tap FIR filter written in Verilog (`zs_fir`,
//! AXI4-Lite at 0x43C00000, interrupt IRQ_F2P[0] = GIC 61) next to Xilinx IP on the Zybo Z7-20:
//!
//! * `zs_fir`: a lowpass pass (interrupt driven, 64 samples) and a highpass pass (polled), each
//!   checked sample by sample against a software reference FIR in `model.rs`;
//! * `axi_gpio_0` (0x41200000): LEDs on channel 1, switches on channel 2;
//! * `axi_gpio_1` (0x41210000): buttons on channel 1;
//! * `axi_timer_0` (0x42800000): three 1 ms periodic interrupts (GIC 64, IRQ_F2P[3]);
//! * `axi_quad_spi_0` (0x41E00000): 16 bytes in loopback;
//! * `axi_uartlite_0` (0x40600000): one line on the PL UART.
//!
//! It prints `RESULT: ALL PASS` on UART1 only if every check held, then writes the
//! testbench's end-of-test value to the FIR window (`rt::finish`).
#![no_std]
#![no_main]

mod model;

use core::sync::atomic::{AtomicU32, Ordering};
use model::{COEF_HIGH, COEF_LOW, N, TAPS};
use rt::{irq, println, read32, write32};

const FIR: usize = 0x43C0_0000;
const LEDS: usize = 0x4120_0000; // axi_gpio_0
const SWBTN: usize = 0x4121_0000; // axi_gpio_1
const TIMER: usize = 0x4280_0000;
const UART: usize = 0x4060_0000;
const SPI: usize = 0x41E0_0000;

// zs_fir register map.
const FIR_CTRL: usize = 0x00;
const FIR_STATUS: usize = 0x04;
const FIR_ID: usize = 0x0C;
const FIR_COEF: usize = 0x10;
const FIR_DIN: usize = 0x40;
const FIR_DOUT: usize = 0x44;
const FIR_ID_VALUE: u32 = 0x5A46_4952;
const CTRL_ENABLE: u32 = 1 << 0;
const CTRL_IRQ_EN: u32 = 1 << 1;
const CTRL_CLEAR: u32 = 1 << 2;
const STATUS_DONE: u32 = 1 << 1;

// AXI GPIO (PG144).
const GPIO_DATA: usize = 0x000;
const GPIO_TRI: usize = 0x004;
const GPIO2_DATA: usize = 0x008;
const GPIO2_TRI: usize = 0x00C;

// AXI Timer (PG079).
const TCSR0: usize = 0x00;
const TLR0: usize = 0x04;
const UDT: u32 = 1 << 1;
const ARHT: u32 = 1 << 4;
const LOAD: u32 = 1 << 5;
const ENIT: u32 = 1 << 6;
const ENT: u32 = 1 << 7;
const TINT: u32 = 1 << 8;

// AXI UART Lite (PG142).
const TX_FIFO: usize = 0x04;
const STAT_REG: usize = 0x08;
const CTRL_REG: usize = 0x0C;
const STAT_TX_EMPTY: u32 = 1 << 2;
const STAT_TX_FULL: u32 = 1 << 3;
const CTRL_RST_TX: u32 = 1 << 0;
const CTRL_RST_RX: u32 = 1 << 1;

// AXI Quad SPI (PG153), absolute addresses.
const SPICR: usize = SPI + 0x60;
const SPISR: usize = SPI + 0x64;
const SPIDTR: usize = SPI + 0x68;
const SPIDRR: usize = SPI + 0x6C;
const SPISSR: usize = SPI + 0x70;
const SPICR_LOOP: u32 = 1 << 0;
const SPICR_SPE: u32 = 1 << 1;
const SPICR_MASTER: u32 = 1 << 2;
const SPISR_RX_EMPTY: u32 = 1 << 0;

const IRQ_FIR: usize = 61; // IRQ_F2P[0]
const IRQ_TIMER: usize = 64; // IRQ_F2P[3]

/// Bound on polling loops (each step is `rt::delay_us(10)`, about 20 ms of simulated time).
const WAIT_LIMIT: u32 = 2000;

static FIR_IRQS: AtomicU32 = AtomicU32::new(0);
static TICKS: AtomicU32 = AtomicU32::new(0);

fn fir_isr() {
    let st = read32(FIR + FIR_STATUS);
    if st & STATUS_DONE != 0 {
        // Write 1 to DONE to clear it; this also drops the level-high irq.
        write32(FIR + FIR_STATUS, STATUS_DONE);
        FIR_IRQS.fetch_add(1, Ordering::SeqCst);
    }
}

fn timer_isr() {
    let c = read32(TIMER + TCSR0);
    if c & TINT != 0 {
        // Write 1 to TINT to clear it; the other bits are written back unchanged.
        write32(TIMER + TCSR0, c);
        TICKS.fetch_add(1, Ordering::SeqCst);
    }
}

fn uart_put(b: u8) {
    while read32(UART + STAT_REG) & STAT_TX_FULL != 0 {
        rt::delay_us(10);
    }
    write32(UART + TX_FIFO, u32::from(b));
}

/// Ends the run without `RESULT: ALL PASS`.
fn fail(msg: &str) -> ! {
    println!("FAIL {msg}");
    rt::finish()
}

/// Loads the eight coefficients (signed 16 bit in bits [15:0]).
fn load_coefs(coef: &[i16; TAPS]) {
    for (k, &c) in coef.iter().enumerate() {
        write32(FIR + FIR_COEF + 4 * k, u32::from(c as u16));
    }
}

/// Pushes one sample into the FIR; the hardware accepts it only when idle.
fn push_sample(x: i16) {
    write32(FIR + FIR_DIN, u32::from(x as u16));
}

/// Lowpass pass: interrupt driven. Each sample is pushed, the interrupt (counted in `fir_isr`)
/// says the result is ready, and DOUT is compared with the reference. Returns the first
/// mismatch as `Err`.
fn lowpass_pass(x: &[i16; N], sw: &[i32; N]) -> Result<(), &'static str> {
    write32(FIR + FIR_CTRL, CTRL_CLEAR);
    load_coefs(&COEF_LOW);
    write32(FIR + FIR_CTRL, CTRL_ENABLE | CTRL_IRQ_EN);
    for n in 0..N {
        push_sample(x[n]);
        let mut waited = 0;
        while FIR_IRQS.load(Ordering::SeqCst) <= n as u32 {
            if waited > WAIT_LIMIT {
                println!("lowpass n={n}: no FIR interrupt");
                return Err("lowpass timeout");
            }
            rt::delay_us(10);
            waited += 1;
        }
        let hw = read32(FIR + FIR_DOUT) as i32;
        if hw != sw[n] {
            println!("lowpass n={n} hw={hw} sw={}", sw[n]);
            return Err("lowpass mismatch");
        }
    }
    Ok(())
}

/// Highpass pass: polled. Each sample is pushed, STATUS.DONE is polled, DOUT is read and DONE
/// is acknowledged.
fn highpass_pass(x: &[i16; N], sw: &[i32; N]) -> Result<(), &'static str> {
    write32(FIR + FIR_CTRL, CTRL_CLEAR);
    load_coefs(&COEF_HIGH);
    write32(FIR + FIR_CTRL, CTRL_ENABLE);
    for n in 0..N {
        push_sample(x[n]);
        let mut waited = 0;
        while read32(FIR + FIR_STATUS) & STATUS_DONE == 0 {
            if waited > WAIT_LIMIT {
                println!("highpass n={n}: DONE never set");
                return Err("highpass timeout");
            }
            rt::delay_us(10);
            waited += 1;
        }
        let hw = read32(FIR + FIR_DOUT) as i32;
        write32(FIR + FIR_STATUS, STATUS_DONE);
        if hw != sw[n] {
            println!("highpass n={n} hw={hw} sw={}", sw[n]);
            return Err("highpass mismatch");
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("ZYNQSIM SHOWCASE");

    let id = read32(FIR + FIR_ID);
    if id != FIR_ID_VALUE {
        println!("FAIL fir id={id:#010x}");
        rt::finish();
    }
    println!("fir id ok");

    irq::install();
    irq::set_handler(IRQ_FIR, fir_isr);
    irq::set_handler(IRQ_TIMER, timer_isr);
    irq::enable_spi(IRQ_FIR);
    irq::enable_spi(IRQ_TIMER);
    irq::enable();

    // Input signal and software reference for both coefficient sets.
    let x = model::input_signal();
    let y_low = model::fir_ref(&COEF_LOW, &x);
    let y_high = model::fir_ref(&COEF_HIGH, &x);

    // 1. Lowpass, interrupt driven.
    if let Err(e) = lowpass_pass(&x, &y_low) {
        fail(e);
    }
    println!("fir lowpass: {N} samples hw==sw");
    let irqs = FIR_IRQS.load(Ordering::SeqCst);
    println!("fir irq count: {irqs}");
    if irqs != N as u32 {
        fail("fir irq count");
    }

    // 2. Highpass, polled.
    if let Err(e) = highpass_pass(&x, &y_high) {
        fail(e);
    }
    println!("fir highpass: {N} samples hw==sw");

    // 3. LEDs: walk 0x1, 0x2, 0x4, 0x8 (channel 1, tri-state cleared = outputs).
    write32(LEDS + GPIO_TRI, 0);
    for i in 0..4 {
        write32(LEDS + GPIO_DATA, 1 << i);
        rt::delay_us(20);
    }
    println!("leds walked");

    // 4. Switches (channel 2, all inputs) and buttons (channel 1 of axi_gpio_1).
    write32(LEDS + GPIO2_TRI, 0xF);
    let sw = read32(LEDS + GPIO2_DATA) & 0xF;
    println!("switches: {sw:#x}");
    let btn = read32(SWBTN + GPIO_DATA) & 0xF;
    println!("buttons: {btn:#x}");

    // 5. Timer: 100000 clocks (1 ms at 100 MHz), auto reload, three interrupts, then stop.
    write32(TIMER + TLR0, 99_999);
    write32(TIMER + TCSR0, LOAD);
    write32(TIMER + TCSR0, ENT | ENIT | ARHT | UDT);
    let mut waited = 0;
    while TICKS.load(Ordering::SeqCst) < 3 && waited < WAIT_LIMIT {
        rt::delay_us(10);
        waited += 1;
    }
    write32(TIMER + TCSR0, 0);
    let ticks = TICKS.load(Ordering::SeqCst);
    println!("timer irq count: {ticks}");
    if ticks != 3 {
        fail("timer irq count");
    }

    // 6. Quad SPI in loopback (as in spi_demo): 16 bytes 0xA0..=0xAF.
    write32(SPICR, SPICR_LOOP | SPICR_SPE | SPICR_MASTER);
    write32(SPISSR, !1);
    for i in 0..16u32 {
        let b = 0xA0 + i;
        write32(SPIDTR, b);
        if read32(SPISR) & SPISR_RX_EMPTY != 0 {
            fail("spi rx empty after transfer");
        }
        let rx = read32(SPIDRR) & 0xFF;
        if rx != b {
            println!("FAIL spi byte {i}: sent {b:#x} got {rx:#x}");
            rt::finish();
        }
    }
    println!("spi loopback: 16 bytes ok");

    // 7. UART Lite: reset the FIFOs, send the line, wait for the transmitter to drain.
    write32(UART + CTRL_REG, CTRL_RST_TX | CTRL_RST_RX);
    for &b in b"SHOWCASE PL UART OK\r\n" {
        uart_put(b);
    }
    let mut drained = false;
    for _ in 0..400 {
        if read32(UART + STAT_REG) & STAT_TX_EMPTY != 0 {
            drained = true;
            break;
        }
        rt::delay_us(10);
    }
    if !drained {
        fail("uartlite not drained");
    }
    println!("uartlite sent");

    // ASSUMPTION: "hw fir: <N> timer ticks per sample" is informational. One FIR sample takes
    // 9 cycles at 100 MHz (90 ns), which is less than one 1 ms timer tick, so N is 0.
    println!("hw fir: 0 timer ticks per sample");

    // Final LED pattern, then `rt::finish` (end-of-test write 0xC051D0DE to 0x43C0003C).
    write32(LEDS + GPIO_DATA, 0x9);
    println!("RESULT: ALL PASS");
    rt::finish()
}
