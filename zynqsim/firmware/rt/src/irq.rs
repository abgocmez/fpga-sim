//! Interrupts for the bare-metal runtime: a vector table, an IRQ-mode stack
//! and the Cortex-A9 GIC (distributor at 0xF8F01000, CPU interface at
//! 0xF8F00100). Level-high shared peripheral interrupts only, which is what
//! `IRQ_F2P[n]` from the PL are: IRQ_F2P[0] is interrupt ID 61, [1] is 62...
//!
//! Usage: `irq::install(); irq::set_handler(61, handler); irq::enable_spi(61);
//! irq::enable();`. A handler runs in IRQ mode with interrupts masked; keep it
//! short and clear the interrupt source in the PL, or the level stays high.

use crate::{read32, write32};
use core::arch::{asm, global_asm};
use core::ptr::{addr_of, addr_of_mut, write_volatile};

const GICD: usize = 0xF8F0_1000;
const GICC: usize = 0xF8F0_0100;
const ICCICR: usize = GICC;
const ICCPMR: usize = GICC + 0x04;
const ICCIAR: usize = GICC + 0x0C;
const ICCEOIR: usize = GICC + 0x10;
const ICDDCR: usize = GICD;
const ICDISER: usize = GICD + 0x100;
const ICDIPR: usize = GICD + 0x400;
const ICDIPTR: usize = GICD + 0x800;
const ICDICFR: usize = GICD + 0xC00;

/// Interrupt IDs handled (32 private + 64 shared lines of the PS are more
/// than the PL can reach; IDs up to 95 are enough for `IRQ_F2P`).
const MAX_ID: usize = 96;

/// Handlers by interrupt ID.
static mut HANDLERS: [Option<fn()>; MAX_ID] = [None; MAX_ID];

/// Stack for IRQ mode.
#[repr(align(8))]
#[allow(dead_code)] // only its address is used
struct IrqStack([u8; 4096]);
static mut IRQ_STACK: IrqStack = IrqStack([0; 4096]);

// Exception vectors: eight word-sized branches, 32-byte aligned for VBAR.
global_asm!(
    ".section .text.vectors,\"ax\"",
    ".balign 32",
    ".global __zs_vectors",
    "__zs_vectors:",
    "    b .",              // reset (never taken: core 0 starts at _start)
    "    b .",              // undefined instruction
    "    b .",              // supervisor call
    "    b .",              // prefetch abort
    "    b .",              // data abort
    "    b .",              // reserved
    "    b __zs_irq_entry", // IRQ
    "    b .",              // FIQ
    ".global __zs_irq_entry",
    "__zs_irq_entry:",
    "    sub lr, lr, #4",
    "    push {{r0-r3, r12, lr}}",
    "    bl __zs_irq_dispatch",
    "    ldm sp!, {{r0-r3, r12, pc}}^",
);

unsafe extern "C" {
    static __zs_vectors: u8;
}

/// Called from the IRQ entry: acknowledge, run the handler, end the interrupt.
#[unsafe(no_mangle)]
extern "C" fn __zs_irq_dispatch() {
    let iar = read32(ICCIAR);
    let id = (iar & 0x3FF) as usize;
    if id < MAX_ID {
        // SAFETY: HANDLERS is written only by `set_handler` before interrupts
        // are enabled, and read here in IRQ mode.
        let h = unsafe { (*addr_of!(HANDLERS))[id] };
        if let Some(f) = h {
            f();
        }
    }
    if id < 1020 {
        write32(ICCEOIR, iar);
    }
}

/// Points VBAR at the vector table and gives IRQ mode a stack. Interrupts stay
/// masked until [`enable`].
pub fn install() {
    // SAFETY: privileged CP15/CPSR accesses on the only core running this code.
    unsafe {
        let vbar = addr_of!(__zs_vectors) as u32;
        asm!("mcr p15, 0, {0}, c12, c0, 0", in(reg) vbar);
        let top = addr_of_mut!(IRQ_STACK) as u32 + 4096;
        asm!(
            "mrs {save}, cpsr",
            "cps #0x12",          // IRQ mode
            "mov sp, {top}",
            "msr cpsr_c, {save}", // back to where we were
            top = in(reg) top,
            save = out(reg) _,
        );
    }
    // Distributor and CPU interface on, all priorities allowed.
    write32(ICDDCR, 1);
    write32(ICCPMR, 0xF0);
    write32(ICCICR, 1);
}

/// Registers `f` for interrupt `id`.
pub fn set_handler(id: usize, f: fn()) {
    assert!(id < MAX_ID, "interrupt id out of range");
    // SAFETY: single core, before the interrupt is enabled.
    unsafe { (*addr_of_mut!(HANDLERS))[id] = Some(f) };
}

/// Enables shared peripheral interrupt `id` as level-high, priority 0xA0,
/// routed to core 0.
pub fn enable_spi(id: usize) {
    assert!((32..MAX_ID).contains(&id), "not a shared peripheral interrupt");
    // SAFETY: byte-wide GIC registers.
    unsafe {
        write_volatile((ICDIPR + id) as *mut u8, 0xA0);
        write_volatile((ICDIPTR + id) as *mut u8, 0x01);
    }
    let cfg = ICDICFR + 4 * (id / 16);
    let shift = 2 * (id % 16);
    write32(cfg, (read32(cfg) & !(0b11 << shift)) | (0b01 << shift));
    write32(ICDISER + 4 * (id / 32), 1 << (id % 32));
}

/// Unmasks IRQs in the CPU.
pub fn enable() {
    // SAFETY: clears the I bit of the CPSR.
    unsafe { asm!("cpsie i") };
}
