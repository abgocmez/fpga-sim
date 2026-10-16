//! Xilinx IP demo (`examples/gpio-timer-uart`): AXI GPIO, AXI Timer and AXI UART Lite, as the
//! block design in `fixtures/xsa/zs_vendor_ip.xsa` places them.
//!
//! * `axi_gpio_0` (0x41200000): four outputs, walked over the board LEDs;
//! * `axi_gpio_1` (0x41210000): channel 1 = four switches, channel 2 = four buttons, with the
//!   interrupt on input change (IRQ_F2P[1], GIC 62);
//! * `axi_timer_0` (0x42800000): periodic down counter, interrupt IRQ_F2P[0] (GIC 61);
//! * `axi_uartlite_0` (0x40600000): a greeting out, and every byte received is echoed.
//!
//! After the self test the firmware mirrors the switches onto the LEDs (all LEDs while a
//! button is held) and reports changes on UART1.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const LEDS: usize = 0x4120_0000;
const SWBTN: usize = 0x4121_0000;
const TIMER: usize = 0x4280_0000;
const UART: usize = 0x4060_0000;

// AXI GPIO (PG144).
const GPIO_DATA: usize = 0x000;
const GPIO2_DATA: usize = 0x008;
const GIER: usize = 0x11C;
const IP_ISR: usize = 0x120;
const IP_IER: usize = 0x128;

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
const RX_FIFO: usize = 0x00;
const TX_FIFO: usize = 0x04;
const STAT_REG: usize = 0x08;
const CTRL_REG: usize = 0x0C;
const STAT_RX_VALID: u32 = 1 << 0;
const STAT_TX_FULL: u32 = 1 << 3;
const CTRL_RST_TX: u32 = 1 << 0;
const CTRL_RST_RX: u32 = 1 << 1;

const IRQ_TIMER: usize = 61; // IRQ_F2P[0]
const IRQ_GPIO: usize = 62; // IRQ_F2P[1]

static TICKS: AtomicU32 = AtomicU32::new(0);
static GPIO_EVENTS: AtomicU32 = AtomicU32::new(0);

fn timer_isr() {
    let c = read32(TIMER + TCSR0);
    if c & TINT != 0 {
        // Write 1 to TINT to clear it; the other bits are written back unchanged.
        write32(TIMER + TCSR0, c);
        TICKS.fetch_add(1, Ordering::SeqCst);
    }
}

fn gpio_isr() {
    let isr = read32(SWBTN + IP_ISR);
    if isr != 0 {
        // Toggle on write: writing back the set bits clears them.
        write32(SWBTN + IP_ISR, isr);
        GPIO_EVENTS.fetch_add(1, Ordering::SeqCst);
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
    println!("gpio-timer-uart demo");

    // LED walk.
    for i in 0..4 {
        write32(LEDS + GPIO_DATA, 1 << i);
        rt::delay_us(20);
        println!("led walk {:#x} reads {:#x}", 1 << i, read32(LEDS + GPIO_DATA));
    }
    write32(LEDS + GPIO_DATA, 0);

    // Timer: 5000 clocks (50 us at 100 MHz) down counter with auto reload, three interrupts.
    irq::install();
    irq::set_handler(IRQ_TIMER, timer_isr);
    irq::set_handler(IRQ_GPIO, gpio_isr);
    irq::enable_spi(IRQ_TIMER);
    irq::enable_spi(IRQ_GPIO);
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

    // UART Lite: reset the FIFOs and send a greeting.
    write32(UART + CTRL_REG, CTRL_RST_TX | CTRL_RST_RX);
    for &b in b"Hello from AXI UART Lite\r\n" {
        uart_put(b);
    }
    println!("uartlite greeting queued");

    // GPIO interrupt on the switches and buttons.
    write32(SWBTN + IP_ISR, read32(SWBTN + IP_ISR)); // clear what the reset state left
    write32(SWBTN + IP_IER, 0b11);
    write32(SWBTN + GIER, 0x8000_0000);

    if ticks == 3 {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    // Mirror loop.
    let mut last = u32::MAX;
    loop {
        let sw = read32(SWBTN + GPIO_DATA) & 0xF;
        let btn = read32(SWBTN + GPIO2_DATA) & 0xF;
        let v = sw | (btn << 4);
        write32(LEDS + GPIO_DATA, if btn != 0 { 0xF } else { sw });
        if v != last {
            last = v;
            println!(
                "sw={sw:#x} btn={btn:#x} gpio_irqs={}",
                GPIO_EVENTS.load(Ordering::SeqCst)
            );
        }
        while read32(UART + STAT_REG) & STAT_RX_VALID != 0 {
            let b = read32(UART + RX_FIFO) as u8;
            uart_put(b);
        }
        rt::delay_us(20);
    }
}
