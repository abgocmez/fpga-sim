//! Dual-core demo: the same ELF runs on both cores of the Zynq PS.
//!
//! Core 0 releases core 1 through the boot protocol: it writes core 1's entry address to the
//! mailbox at 0xFFFFFFF0 and sends an event (SEV). Then both cores, in order:
//! * increment a shared counter 500 times each under an LDREX/STREX spinlock (a waiting core
//!   sleeps in WFE, the holder sends SEV when it releases the lock);
//! * ping-pong 50 rounds on two shared words, each core waiting in WFE for the other;
//! * core 0 raises SGI 1 for core 1, whose interrupt handler runs with IRQs enabled.
//!
//! Core 0 prints on UART1 (the runtime's console), core 1 on UART0. The last line of UART1 is
//! `RESULT: ALL PASS` when every check holds. Core 1 reaches its code through the same
//! `_start` as core 0 on the QEMU engine (where it begins at the ELF entry and parks on the
//! mailbox), and through the boot ROM's parking loop on the native engine.
#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::fmt::{self, Write};
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use rt::irq;
use rt::{println, read32, write32};

/// Where core 0 writes core 1's entry address.
const MAILBOX: usize = 0xFFFF_FFF0;
/// Distributor: SGI 1 enable (banked per core), priority, and the software-generated interrupt.
const GICD_ISENABLER0: usize = 0xF8F0_1100;
const GICD_IPRIORITYR: usize = 0xF8F0_1400;
const GICD_SGIR: usize = 0xF8F0_1F00;
/// UART0, the console of core 1 (UART1 is the runtime's).
const UART0: usize = 0xE000_0000;
const UART_SR: usize = 0x2C;
const UART_FIFO: usize = 0x30;
const SR_TXFULL: u32 = 1 << 4;

const INCREMENTS: u32 = 500;
const ROUNDS: u32 = 50;

/// Shared words, as indexes into `SHARED` (the cores see the same memory).
const LOCK: usize = 0;
const COUNTER: usize = 1;
const READY: usize = 2;
const CORE1_DONE: usize = 3;
const PING: usize = 4;
const PONG: usize = 5;
const SGI_SEEN: usize = 6;
const SGI_DONE: usize = 7;
const WORDS: usize = 8;

static mut SHARED: [u32; WORDS] = [0; WORDS];

/// The address of the lock word.
fn lock_word() -> *mut u32 {
    // SAFETY: an index into SHARED (the pointer is only computed, then used by the asm).
    unsafe { (addr_of_mut!(SHARED) as *mut u32).add(LOCK) }
}

fn get(i: usize) -> u32 {
    // SAFETY: an index into SHARED, read with a volatile access (the other core writes it).
    unsafe { read_volatile((addr_of!(SHARED) as *const u32).add(i)) }
}

fn set(i: usize, v: u32) {
    // SAFETY: as in `get`.
    unsafe { write_volatile((addr_of_mut!(SHARED) as *mut u32).add(i), v) }
}

/// An event: wakes every core waiting in WFE.
fn sev() {
    // SAFETY: a hint instruction.
    unsafe { asm!("sev") }
}

/// Sleeps in WFE until shared word `i` holds `want`.
fn wait_until(i: usize, want: u32) {
    while get(i) != want {
        // SAFETY: a hint instruction; it wakes on the other core's SEV.
        unsafe { asm!("wfe") }
    }
}

// The spinlock: LDREX/STREX on the lock word. A core that finds it held sleeps in a
// conditional WFE (NE) until the holder's SEV, then tries again. `zs_spin_unlock` stores 0
// and sends the event. Both are A32 and take the lock word in r0.
global_asm!(
    ".section .text.zs_lock,\"ax\"",
    ".global zs_spin_lock",
    "zs_spin_lock:",
    "    mov r1, #1",
    "1:  ldrex r2, [r0]",
    "    cmp r2, #0",
    "    wfene",
    "    bne 1b",
    "    strex r2, r1, [r0]",
    "    cmp r2, #0",
    "    bne 1b",
    "    bx lr",
    ".global zs_spin_unlock",
    "zs_spin_unlock:",
    "    mov r1, #0",
    "    str r1, [r0]",
    "    sev",
    "    bx lr",
);

unsafe extern "C" {
    fn zs_spin_lock(lock: *mut u32);
    fn zs_spin_unlock(lock: *mut u32);
    /// Core 1's first instruction after the mailbox release (see `global_asm` below).
    fn core1_entry();
}

/// Adds one to the shared counter under the lock.
fn locked_increment() {
    // SAFETY: the lock word is in SHARED; the asm routines follow the AAPCS.
    unsafe { zs_spin_lock(lock_word()) };
    set(COUNTER, get(COUNTER) + 1);
    // SAFETY: as above; this core holds the lock.
    unsafe { zs_spin_unlock(lock_word()) };
}

// Core 1's entry, reached from the mailbox with no stack: set the stack, call the Rust
// entry. The stack is 4 KiB of .bss, after the runtime's stack-free boot path.
global_asm!(
    ".section .bss.core1_stack,\"aw\",%nobits",
    ".balign 8",
    "core1_stack:",
    ".space 4096",
    "core1_stack_top:",
    ".section .text.core1_entry,\"ax\"",
    ".global core1_entry",
    "core1_entry:",
    "    ldr sp, =core1_stack_top",
    "    bl {core1_main}",
    "2:  wfi",
    "    b 2b",
    core1_main = sym core1_main,
);

/// UART0 for core 1 (`print` through `writeln!`).
struct Uart0;

fn putc0(b: u8) {
    while read32(UART0 + UART_SR) & SR_TXFULL != 0 {}
    write32(UART0 + UART_FIFO, u32::from(b));
}

impl Write for Uart0 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                putc0(b'\r');
            }
            putc0(b);
        }
        Ok(())
    }
}

/// The interrupt handler of SGI 1 (runs on core 1, in IRQ mode).
fn sgi_handler() {
    set(SGI_SEEN, get(SGI_SEEN) + 1);
}

/// Core 0's entry, after the runtime's `_start`.
#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("dual-core demo: core 0 (MPIDR 0) starts");
    irq::install();
    write32(MAILBOX, core1_entry as *const () as usize as u32);
    sev();
    wait_until(READY, 1);
    println!("core 0: core 1 is up; both cores increment the counter {INCREMENTS} times");

    for _ in 0..INCREMENTS {
        locked_increment();
    }
    wait_until(CORE1_DONE, 1);
    let counter = get(COUNTER);
    let counter_ok = counter == 2 * INCREMENTS;
    println!(
        "core 0: counter {counter} (expected {}): {}",
        2 * INCREMENTS,
        if counter_ok { "PASS" } else { "FAIL" }
    );

    for n in 1..=ROUNDS {
        set(PING, n);
        sev();
        wait_until(PONG, n);
    }
    println!("core 0: {ROUNDS} ping-pong rounds with WFE and SEV");

    // SGI 1 to core 1 only (target list bit 1, filter 0).
    write32(GICD_SGIR, (0b10 << 16) | 1);
    wait_until(SGI_DONE, 1);
    let sgi_ok = get(SGI_SEEN) == 1;
    println!(
        "core 0: SGI 1 delivered to core 1 once: {}",
        if sgi_ok { "PASS" } else { "FAIL" }
    );

    if counter_ok && sgi_ok {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAILED");
    }
    loop {
        // SAFETY: parks the core until an interrupt that never comes.
        unsafe { asm!("wfi") }
    }
}

/// Core 1's software: it sets up its GIC interface, announces itself, then does the same
/// work as core 0.
#[unsafe(no_mangle)]
pub extern "C" fn core1_main() -> ! {
    // The vector table, IRQ stack, distributor and this core's CPU interface (banked).
    irq::install();
    irq::set_handler(1, sgi_handler);
    write32(GICD_ISENABLER0, 1 << 1);
    // SAFETY: a byte-wide GIC priority register.
    unsafe { write_volatile((GICD_IPRIORITYR + 1) as *mut u8, 0xA0) };
    irq::enable();
    let _ = writeln!(Uart0, "core 1 (MPIDR 1): released through 0xFFFFFFF0");
    set(READY, 1);
    sev();

    for _ in 0..INCREMENTS {
        locked_increment();
    }
    let _ = writeln!(Uart0, "core 1: counter increments done");
    set(CORE1_DONE, 1);
    sev();

    for n in 1..=ROUNDS {
        wait_until(PING, n);
        set(PONG, n);
        sev();
    }
    while get(SGI_SEEN) == 0 {
        // SAFETY: sleeps until an interrupt; the SGI handler sets SGI_SEEN.
        unsafe { asm!("wfi") }
    }
    let _ = writeln!(Uart0, "core 1: SGI 1 received, ping-pong done");
    set(SGI_DONE, 1);
    sev();
    loop {
        // SAFETY: as above.
        unsafe { asm!("wfi") }
    }
}
