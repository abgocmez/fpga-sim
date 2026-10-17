//! The register sequences of Xilinx standalone-BSP drivers, replayed against the models, and the
//! watchdog and private timer behaviour the request asked for.
//!
//! * `XCanPs_SelfTest` (`fixtures/bsp/bsp_canps_polled.elf`): reset, configuration mode, loopback
//!   (mode code 4), one frame sent, received back word for word, the RX FIFO popped and the RXOK
//!   flag cleared. The register order is the one the disassembly of the ELF shows.
//! * `XDcfg_SelfTest` (`fixtures/bsp/bsp_devcfg_selftest.elf`): the self test sets CTRL bit 4
//!   (NIDEN) and passes when the bit reads back set.
//! * The SCU watchdog in watchdog mode makes a reset request that the host takes; in timer mode
//!   it only flags its event. The private timer counts at 333.33 MHz (CPU clock / 2).

mod support;

use mem_sys::SystemBus;
use support::*;

const CAN0: u64 = 0xE000_8000;
const SRR: u64 = 0x00;
const MSR: u64 = 0x04;
const BRPR: u64 = 0x08;
const BTR: u64 = 0x0C;
const SR: u64 = 0x18;
const ISR: u64 = 0x1C;
const ICR: u64 = 0x24;
const TXFIFO: u64 = 0x30;
const RXFIFO: u64 = 0x50;

const DEVCFG_CTRL: u64 = 0xF800_7000;
const NIDEN: u32 = 1 << 4;

const WDT: u64 = 0xF8F0_0620;
const WDT_LOAD: u64 = WDT;
const WDT_COUNTER: u64 = WDT + 0x04;
const WDT_CONTROL: u64 = WDT + 0x08;
const WDT_RESET_STATUS: u64 = WDT + 0x10;
const CTRL_EN: u32 = 1 << 0;
const CTRL_AR: u32 = 1 << 1;
const CTRL_WD_MODE: u32 = 1 << 3;

const PT_LOAD: u64 = 0xF8F0_0600;
const PT_COUNTER: u64 = PT_LOAD + 0x04;
const PT_CONTROL: u64 = PT_LOAD + 0x08;

/// The mode code `XCanPs_GetMode` returns, decoded from the status register.
fn can_mode(bus: &mut SystemBus) -> u32 {
    let sr = rd(bus, CAN0 + SR);
    if sr & 1 != 0 {
        1
    } else if sr & 4 != 0 {
        8
    } else if sr & 8 == 0 {
        4
    } else if sr & 0x1000 != 0 {
        16
    } else {
        2
    }
}

#[test]
fn xcanps_selftest_sequence_passes_in_loopback() {
    let (mut bus, _h) = system();
    // XCanPs_Reset: SRR = SRST, then GetMode must be configuration (1).
    wr(&mut bus, CAN0 + SRR, 1);
    assert_eq!(can_mode(&mut bus), 1, "configuration after reset");
    // SetBaudRatePrescaler(29), SetBitTiming(3, 15, 2).
    wr(&mut bus, CAN0 + BRPR, 29);
    wr(&mut bus, CAN0 + BTR, (3 << 7) | (15 << 2) | 2);
    // EnterMode(4): SRR = 0, MSR = 2 (loopback), SRR = 2 (leave configuration).
    wr(&mut bus, CAN0 + SRR, 0);
    wr(&mut bus, CAN0 + MSR, 2);
    wr(&mut bus, CAN0 + SRR, 2);
    assert_eq!(can_mode(&mut bus), 4, "loopback");
    // XCanPs_Send: the frame words; words 2 and 3 are byte swapped by the driver.
    let frame: [u32; 4] = [0xFA00_0000, 0x8000_0000, 0, 0];
    for (k, w) in frame.iter().enumerate() {
        let v = if k >= 2 { w.swap_bytes() } else { *w };
        wr(&mut bus, CAN0 + TXFIFO + 4 * k as u64, v);
    }
    // The self test polls ISR bit 7 (TXOK) before receiving.
    assert_ne!(rd(&mut bus, CAN0 + ISR) & 0x80, 0);
    // XCanPs_Recv: the RX FIFO words, the last two byte swapped back.
    let rx: Vec<u32> = (0..4)
        .map(|k| {
            let v = rd(&mut bus, CAN0 + RXFIFO + 4 * k as u64);
            if k >= 2 { v.swap_bytes() } else { v }
        })
        .collect();
    assert_eq!(rx[0], 0xFA00_0000, "ID/DLC word");
    assert_eq!(rx[1] & 0xFFFF_0000, 0x8000_0000, "the self test compares the top half");
    assert_eq!(rx[2], 0);
    assert_eq!(rx[3], 0);
    // XCanPs_IntrClear(0x80), then the frame is gone.
    wr(&mut bus, CAN0 + ICR, 0x80);
    assert_eq!(rd(&mut bus, CAN0 + ISR) & 0x80, 0);
    // XCanPs_Reset at the end of the self test.
    wr(&mut bus, CAN0 + SRR, 1);
    assert_eq!(can_mode(&mut bus), 1);
}

#[test]
fn xdcfg_selftest_passes_when_nidien_reads_back_set() {
    let (mut bus, _h) = system();
    let before = rd(&mut bus, DEVCFG_CTRL);
    // XDcfg_SetControlRegister(NIDEN), then GetControlRegister.
    wr(&mut bus, DEVCFG_CTRL, before | NIDEN);
    let after = rd(&mut bus, DEVCFG_CTRL);
    // The self test returns ((after ^ NIDEN) >> 4) & 1: zero means success.
    assert_eq!((after ^ NIDEN) >> 4 & 1, 0);
    wr(&mut bus, DEVCFG_CTRL, before);
}

#[test]
fn a_watchdog_in_watchdog_mode_requests_one_reset_per_expiry_and_the_host_takes_it() {
    let (mut bus, h) = system();
    wr(&mut bus, WDT_LOAD, 500);
    wr(&mut bus, WDT_COUNTER, 500);
    wr(&mut bus, WDT_CONTROL, CTRL_EN | CTRL_WD_MODE);
    assert_eq!(h.mpcore.lock().unwrap().take_wdt_resets(), 0, "nothing before expiry");
    // 500 PERIPHCLK ticks = 1000 CPU cycles per expiry.
    bus.tick(1_500);
    assert!(h.mpcore.lock().unwrap().take_wdt_resets() >= 1, "expiry requests a reset");
    assert_eq!(rd(&mut bus, WDT_RESET_STATUS) & 1, 1, "RESET_STATUS set");
    assert_eq!(h.mpcore.lock().unwrap().take_wdt_resets(), 0, "taken once");
    // The reset status is write-1-to-clear.
    wr(&mut bus, WDT_RESET_STATUS, 1);
    assert_eq!(rd(&mut bus, WDT_RESET_STATUS) & 1, 0);
}

#[test]
fn a_watchdog_in_timer_mode_flags_its_event_and_requests_no_reset() {
    let (mut bus, h) = system();
    wr(&mut bus, WDT_LOAD, 500);
    wr(&mut bus, WDT_COUNTER, 500);
    wr(&mut bus, WDT_CONTROL, CTRL_EN | CTRL_AR);
    bus.tick(1_500);
    assert_eq!(h.mpcore.lock().unwrap().take_wdt_resets(), 0);
    assert_eq!(rd(&mut bus, WDT_RESET_STATUS) & 1, 0);
    assert_eq!(rd(&mut bus, WDT + 0x0C) & 1, 1, "the countdown event flag is set");
}

#[test]
fn the_private_timer_counts_at_333_mhz() {
    let (mut bus, _h) = system();
    wr(&mut bus, PT_LOAD, 1_000_000);
    wr(&mut bus, PT_COUNTER, 1_000_000);
    wr(&mut bus, PT_CONTROL, 1); // enable, no prescaler, no interrupt
    // 666 667 CPU cycles is one millisecond at 666.67 MHz: 333 333 PERIPHCLK ticks.
    bus.tick(666_667);
    let counter = rd(&mut bus, PT_COUNTER);
    assert!(
        (666_665..=666_669).contains(&counter),
        "1 ms should count 333 333 ticks, counter {counter}"
    );
}
