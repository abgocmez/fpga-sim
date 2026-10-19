//! AXI Ethernet Lite demonstration (`examples/ethlite-demo`):
//! `axi_ethernetlite_0` at 0x43000000 on M_AXI_GP0, internal loopback enabled,
//! `ip2intc_irpt` on IRQ_F2P[0] (GIC SPI 61).
//!
//! The firmware tests MDIO PHY management, transmits an Ethernet frame into the
//! internal loopback, verifies reception integrity and length, and validates the
//! core interrupt line.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use rt::{irq, println, read32, write32};

const ETHLITE: usize = 0x4300_0000;
const TX_PING_BASE: usize = ETHLITE;
const TX_PING_LEN: usize = ETHLITE + 0x07E4;
const MDIOADDR: usize = ETHLITE + 0x07E8;
const MDIOWR: usize = ETHLITE + 0x07EC;
const MDIORD: usize = ETHLITE + 0x07F0;
const MDIOCTRL: usize = ETHLITE + 0x07F4;
const GIE: usize = ETHLITE + 0x07F8;
const TX_PING_CTRL: usize = ETHLITE + 0x07FC;

const RX_PING_BASE: usize = ETHLITE + 0x1000;
const RX_PING_LEN: usize = ETHLITE + 0x17E4;
const RX_PING_CTRL: usize = ETHLITE + 0x17FC;

const GIE_ENABLE: u32 = 1 << 31;
const TX_CTRL_BUSY: u32 = 1 << 0;
const TX_CTRL_IE: u32 = 1 << 3;
const RX_CTRL_STATUS: u32 = 1 << 0;
const RX_CTRL_IE: u32 = 1 << 3;

const MDIOCTRL_ENABLE: u32 = 1 << 0;
const MDIOCTRL_READY: u32 = 1 << 1;

const IRQ_ETHLITE: usize = 61; // IRQ_F2P[0]

static TX_INTR_COUNT: AtomicU32 = AtomicU32::new(0);
static RX_INTR_COUNT: AtomicU32 = AtomicU32::new(0);

fn ethlite_isr() {
    write32(GIE, 0);
    let rx_ctrl = read32(RX_PING_CTRL);
    if rx_ctrl & RX_CTRL_STATUS != 0 {
        RX_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    let tx_ctrl = read32(TX_PING_CTRL);
    if tx_ctrl & TX_CTRL_BUSY == 0 {
        TX_INTR_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

fn mdio_write(phy_addr: u32, reg_addr: u32, val: u16) {
    write32(MDIOWR, val as u32);
    let cmd = ((phy_addr & 0x1F) << 5) | (reg_addr & 0x1F); // bit 10 is 0 for write
    write32(MDIOADDR, cmd);
    for _ in 0..100 {
        if read32(MDIOCTRL) & MDIOCTRL_READY != 0 {
            break;
        }
        rt::delay_us(1);
    }
}

fn mdio_read(phy_addr: u32, reg_addr: u32) -> u32 {
    let cmd = (1 << 10) | ((phy_addr & 0x1F) << 5) | (reg_addr & 0x1F);
    write32(MDIOADDR, cmd);
    for _ in 0..100 {
        if read32(MDIOCTRL) & MDIOCTRL_READY != 0 {
            break;
        }
        rt::delay_us(1);
    }
    read32(MDIORD)
}

fn test_mdio() -> bool {
    write32(MDIOCTRL, MDIOCTRL_ENABLE);
    let status = mdio_read(1, 1);
    println!("MDIO PHY 1 Reg 1 (Status): {status:#x}");
    let phy_id1 = mdio_read(1, 2);
    let phy_id2 = mdio_read(1, 3);
    println!("MDIO PHY ID: {phy_id1:#06x}:{phy_id2:#06x}");

    // Test MDIO write to Control register (reg 0)
    mdio_write(1, 0, 0x1100);
    let ctrl = mdio_read(1, 0);
    println!("MDIO PHY 1 Reg 0 (Control): {ctrl:#x}");

    // Basic status bit 2 indicates link status (1 = link up)
    ((status & 0x0004) != 0 || status == 0x786D) && (ctrl == 0x1100)
}

fn test_loopback_frame() -> bool {
    const FRAME_LEN: usize = 64;
    let mut tx_frame = [0u8; FRAME_LEN];

    // Destination MAC (00:0A:35:01:02:03)
    tx_frame[0] = 0x00;
    tx_frame[1] = 0x0A;
    tx_frame[2] = 0x35;
    tx_frame[3] = 0x01;
    tx_frame[4] = 0x02;
    tx_frame[5] = 0x03;

    // Source MAC (00:0A:35:AA:BB:CC)
    tx_frame[6] = 0x00;
    tx_frame[7] = 0x0A;
    tx_frame[8] = 0x35;
    tx_frame[9] = 0xAA;
    tx_frame[10] = 0xBB;
    tx_frame[11] = 0xCC;

    // EtherType (0x0800 IPv4)
    tx_frame[12] = 0x08;
    tx_frame[13] = 0x00;

    // Payload
    for (i, byte) in tx_frame.iter_mut().enumerate().take(FRAME_LEN).skip(14) {
        *byte = (i as u8).wrapping_add(0x20);
    }

    // Write frame words into TX Ping buffer memory
    for (i, chunk) in tx_frame.chunks(4).enumerate() {
        let mut word_bytes = [0u8; 4];
        word_bytes.copy_from_slice(chunk);
        let word = u32::from_le_bytes(word_bytes);
        write32(TX_PING_BASE + i * 4, word);
    }

    // Set transmit length
    write32(TX_PING_LEN, FRAME_LEN as u32);

    // Arm RX buffer and enable interrupts
    write32(RX_PING_CTRL, RX_CTRL_IE);
    write32(GIE, GIE_ENABLE);

    // Initiate transmission
    write32(TX_PING_CTRL, TX_CTRL_BUSY | TX_CTRL_IE);

    // Wait for loopback delivery into RX buffer
    let mut rx_ready = false;
    for _ in 0..2000 {
        if read32(RX_PING_CTRL) & RX_CTRL_STATUS != 0 {
            rx_ready = true;
            break;
        }
        rt::delay_us(10);
    }

    if !rx_ready {
        println!("FAIL: RX buffer timed out waiting for frame");
        return false;
    }

    let rx_len = read32(RX_PING_LEN) as usize;
    if rx_len != FRAME_LEN {
        println!("FAIL: RX len mismatch: got {rx_len}, expected {FRAME_LEN}");
        return false;
    }

    // Read back and compare received frame
    let mut rx_frame = [0u8; FRAME_LEN];
    for (i, chunk) in rx_frame.chunks_mut(4).enumerate() {
        let word = read32(RX_PING_BASE + i * 4);
        chunk.copy_from_slice(&word.to_le_bytes());
    }

    let mut mismatch = 0;
    for i in 0..FRAME_LEN {
        if tx_frame[i] != rx_frame[i] {
            mismatch += 1;
        }
    }

    if mismatch > 0 {
        println!("FAIL: {mismatch} bytes differ between TX and RX frames");
        return false;
    }

    // Clear RX buffer and acknowledge TX interrupt
    write32(RX_PING_CTRL, RX_CTRL_IE);
    write32(TX_PING_CTRL, TX_CTRL_IE);

    let rx_intrs = RX_INTR_COUNT.load(Ordering::SeqCst);
    let tx_intrs = TX_INTR_COUNT.load(Ordering::SeqCst);
    println!("Loopback frame verified ({FRAME_LEN} bytes), interrupts: rx={rx_intrs} tx={tx_intrs}");

    rx_intrs > 0 || tx_intrs > 0
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    rt::pl_bringup();
    println!("ethlite demo: AXI Ethernet Lite 3.0 loopback and packet transfer");

    irq::install();
    irq::set_handler(IRQ_ETHLITE, ethlite_isr);
    irq::enable_spi(IRQ_ETHLITE);
    irq::enable();

    let mut pass = true;
    pass &= test_mdio();
    pass &= test_loopback_frame();

    if pass {
        println!("ETHLITE PASS");
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }

    rt::finish()
}
