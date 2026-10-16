//! The firmware of the P3 exit demonstration (`examples/vivado-handoff`): it exercises every
//! piece of the block design in `fixtures/handoff/design.xsa`.
//!
//! * `u_math` (0x43C00000, the user's RTL `my_math_ip`, in XSim): `reg1 = reg0 * 2` for seven
//!   vectors, the same ones as the prototype's `sw/main.c`;
//! * `axi_gpio_0` (0x41200000, four outputs on the board LEDs): a walk, then a pattern;
//! * `axi_timer_0` (0x42800000, interrupt IRQ_F2P[0] = GIC 61): three periodic interrupts;
//! * `axi_uartlite_0` (0x40600000, 115200 8N1): one line, checked through its status register
//!   (the bytes themselves are visible to the host as `uart_read` port 2).
//!
//! It prints `RESULT: ALL PASS` on UART1 only if every check held, and then ends the run with
//! the testbench's end-of-test write (`rt::finish`).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const MATH: usize = 0x43C0_0000;
const LEDS: usize = 0x4120_0000;
const TIMER: usize = 0x4280_0000;
const UART: usize = 0x4060_0000;

// AXI GPIO (PG144).
const GPIO_DATA: usize = 0x000;
const GPIO_TRI: usize = 0x004;

// AXI Timer (PG079).
const TCSR0: usize = 0x00;
const TLR0: usize = 0x04;
const TCR0: usize = 0x08;
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

const IRQ_TIMER: usize = 61; // IRQ_F2P[0]

static TICKS: AtomicU32 = AtomicU32::new(0);

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

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("vivado-handoff demo: math RTL + AXI GPIO + AXI Timer + AXI UART Lite");
    let mut failures = 0u32;

    // 1. The user's RTL.
    const VECTORS: [u32; 7] = [
        0x0000_0000,
        0x0000_0001,
        0x0000_0015,
        0x1234_5678,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
    ];
    for v in VECTORS {
        write32(MATH, v);
        let echo = read32(MATH);
        let out = read32(MATH + 4);
        let expected = v.wrapping_mul(2);
        if echo == v && out == expected {
            println!("math PASS in={v:#010x} out={out:#010x}");
        } else {
            println!("math FAIL in={v:#010x} echo={echo:#010x} out={out:#010x} exp={expected:#010x}");
            failures += 1;
        }
    }

    // 2. GPIO on the LEDs: a walk, then the final pattern.
    for i in 0..4 {
        write32(LEDS + GPIO_DATA, 1 << i);
        let back = read32(LEDS + GPIO_DATA);
        if back != 1 << i {
            println!("gpio FAIL walk {:#x} reads {back:#x}", 1 << i);
            failures += 1;
        }
    }
    write32(LEDS + GPIO_DATA, 0b1001);
    let leds = read32(LEDS + GPIO_DATA);
    let tri = read32(LEDS + GPIO_TRI);
    println!("gpio: tri={tri:#x} data={leds:#x}");
    if leds != 0b1001 || tri != 0 {
        failures += 1;
    }

    // 3. Timer: 5000 clocks (50 us at 100 MHz) down counter, auto reload, three interrupts.
    irq::install();
    irq::set_handler(IRQ_TIMER, timer_isr);
    irq::enable_spi(IRQ_TIMER);
    irq::enable();
    write32(TIMER + TLR0, 4999);
    write32(TIMER + TCSR0, LOAD);
    write32(TIMER + TCSR0, ENT | ENIT | ARHT | UDT);
    let mut waited = 0;
    while TICKS.load(Ordering::SeqCst) < 3 && waited < 2000 {
        rt::delay_us(10);
        waited += 1;
    }
    write32(TIMER + TCSR0, 0);
    let ticks = TICKS.load(Ordering::SeqCst);
    println!("timer ticks={ticks} tcr={}", read32(TIMER + TCR0));
    if ticks != 3 {
        failures += 1;
    }

    // 4. UART Lite: reset the FIFOs, send a line, wait for the transmitter to drain.
    write32(UART + CTRL_REG, CTRL_RST_TX | CTRL_RST_RX);
    for &b in b"HANDOFF UART OK\r\n" {
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
    println!("uartlite line sent, drained={drained}");
    if !drained {
        failures += 1;
    }

    if failures == 0 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: {failures} FAILED");
    }
    rt::finish()
}
