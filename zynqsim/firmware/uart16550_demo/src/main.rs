//! AXI UART 16550 demonstration (`examples/uart16550-demo`):
//! `axi_uart16550_0` at 0x43000000 on M_AXI_GP0, `ip2intc_irpt` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests scratchpad access, baud divisor latch with DLAB, FIFO control,
//! polled loopback communication, and interrupt generation through OUT2 gating.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const UART_BASE: usize = 0x4300_0000;
const REG_OFFSET: usize = 0x1000;

const RBR: usize = UART_BASE + REG_OFFSET;
const THR: usize = UART_BASE + REG_OFFSET;
const DLL: usize = UART_BASE + REG_OFFSET;
const IER: usize = UART_BASE + REG_OFFSET + 0x04;
const DLM: usize = UART_BASE + REG_OFFSET + 0x04;
const IIR: usize = UART_BASE + REG_OFFSET + 0x08;
const FCR: usize = UART_BASE + REG_OFFSET + 0x08;
const LCR: usize = UART_BASE + REG_OFFSET + 0x0C;
const MCR: usize = UART_BASE + REG_OFFSET + 0x10;
const LSR: usize = UART_BASE + REG_OFFSET + 0x14;
const SCR: usize = UART_BASE + REG_OFFSET + 0x1C;

const LCR_8N1: u32 = 0x03;
const LCR_DLAB: u32 = 0x80;

const FCR_FIFO_EN: u32 = 0x01;
const FCR_RX_RST: u32 = 0x02;
const FCR_TX_RST: u32 = 0x04;

const MCR_OUT2: u32 = 0x08;
const MCR_LOOP: u32 = 0x10;

const LSR_DR: u32 = 0x01;
const LSR_THRE: u32 = 0x20;
const LSR_TEMT: u32 = 0x40;

const IER_RX_DATA: u32 = 0x01;

const IRQ_UART: usize = 61; // IRQ_F2P[0]

static RX_INTR_COUNT: AtomicU32 = AtomicU32::new(0);
static RX_INTR_BYTE: AtomicU32 = AtomicU32::new(0);

fn uart_isr() {
    let iir = read32(IIR);
    if (iir & 0x0E) == 0x04 {
        let b = read32(RBR);
        RX_INTR_BYTE.store(b, Ordering::SeqCst);
        RX_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    write32(IER, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("uart16550 demo: AXI UART 16550 2.0 loopback and interrupts");

    let lsr_init = read32(LSR);
    println!("UART16550 LSR reset: {lsr_init:#04x}");
    let mut pass = (lsr_init & (LSR_THRE | LSR_TEMT)) == (LSR_THRE | LSR_TEMT);

    // Test scratchpad register
    write32(SCR, 0xA5);
    pass &= (read32(SCR) & 0xFF) == 0xA5;
    write32(SCR, 0x5A);
    pass &= (read32(SCR) & 0xFF) == 0x5A;
    if pass {
        println!("UART16550 scratchpad ok");
    }

    // Test divisor latch and line control
    write32(LCR, LCR_DLAB);
    write32(DLL, 0x1B);
    write32(DLM, 0x00);
    pass &= (read32(DLL) & 0xFF) == 0x1B;
    pass &= (read32(DLM) & 0xFF) == 0x00;
    write32(LCR, LCR_8N1);
    pass &= (read32(LCR) & 0xFF) == LCR_8N1;
    if pass {
        println!("UART16550 baud divisor and LCR ok");
    }

    // Enable FIFOs and reset
    write32(FCR, FCR_FIFO_EN | FCR_RX_RST | FCR_TX_RST);
    let iir_val = read32(IIR);
    pass &= (iir_val & 0xC0) == 0xC0;
    if pass {
        println!("UART16550 FIFO control ok");
    }

    // Polled loopback mode
    write32(MCR, MCR_LOOP);
    let test_msg = b"16550";
    for &ch in test_msg {
        write32(THR, u32::from(ch));
        let mut timeout = 100_000;
        while (read32(LSR) & LSR_DR) == 0 && timeout > 0 {
            timeout -= 1;
        }
        let rx_byte = (read32(RBR) & 0xFF) as u8;
        pass &= rx_byte == ch;
    }
    if pass {
        println!("UART16550 polled loopback ok");
    }

    // Interrupt handling via GIC SPI 61
    irq::install();
    irq::set_handler(IRQ_UART, uart_isr);
    irq::enable_spi(IRQ_UART);
    irq::enable();

    // Configure MCR with Loopback and OUT2 (enables interrupt line)
    write32(MCR, MCR_LOOP | MCR_OUT2);
    // Enable received data interrupt
    write32(IER, IER_RX_DATA);

    // Send trigger byte
    write32(THR, u32::from(b'Z'));

    let mut timeout = 200_000;
    while RX_INTR_COUNT.load(Ordering::SeqCst) == 0 && timeout > 0 {
        timeout -= 1;
    }
    let rx_irq_count = RX_INTR_COUNT.load(Ordering::SeqCst);
    let rx_irq_byte = (RX_INTR_BYTE.load(Ordering::SeqCst) & 0xFF) as u8;
    pass &= rx_irq_count > 0 && rx_irq_byte == b'Z';

    if pass {
        println!("UART16550 interrupt ok");
        println!("UART16550 PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("UART16550 FAIL");
    }

    rt::finish();
}
