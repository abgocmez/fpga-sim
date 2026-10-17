//! Instruction-level tests. Programs are assembled by the Xilinx GNU toolchain (see
//! `isa_words.inc.rs`, generated from the assembly by `gen_isa.py`) and run from RAM at 0x1000.
//! Expected values are worked out from the ARM ARM by hand in each test.

mod common;

use cpu_a9::{Cpu, Event, Mode, StepResult};
use ps_bus::RamBus;

include!("data/isa_words.inc.rs");

const CODE: u32 = 0x1000;

/// A core with `words` at [`CODE`] in RAM 0x1000..0x4000, in its reset state (SVC, I and F
/// masked, VBAR 0).
struct Machine {
    cpu: Cpu,
    bus: RamBus,
}

fn machine(words: &[u32]) -> Machine {
    let mut bus = RamBus::new(0x1000, 0x3000);
    bus.load(u64::from(CODE), &common::words_to_bytes(words));
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    Machine { cpu, bus }
}

impl Machine {
    fn step(&mut self) -> StepResult {
        self.cpu.step(&mut self.bus)
    }

    /// Steps `n` times, each of which must execute with no event.
    fn run(&mut self, n: usize) {
        for _ in 0..n {
            let r = self.step();
            assert!(
                matches!(r, StepResult::Executed { event: None, .. }),
                "unexpected {r:?} at pc {:#x}",
                self.cpu.pc()
            );
        }
    }

    /// Steps once and returns the event (the step must have executed).
    fn event(&mut self) -> Option<Event> {
        match self.step() {
            StepResult::Executed { event, .. } => event,
            other => panic!("expected an executed step, got {other:?}"),
        }
    }

    fn flags(&self) -> (bool, bool, bool, bool) {
        let c = self.cpu.cpsr();
        (
            c >> 31 & 1 == 1,
            c >> 30 & 1 == 1,
            c >> 29 & 1 == 1,
            c >> 28 & 1 == 1,
        )
    }
}

#[test]
fn branches_bl_bx_and_b_reach_their_targets() {
    // mov; bl sub1; add #1; b end; sub1: add #10; bx lr; end: mov r1, #7
    let mut m = machine(PROG_BRANCH);
    m.run(7);
    assert_eq!(m.cpu.reg(0), 11);
    assert_eq!(m.cpu.reg(1), 7);
}

#[test]
fn all_fifteen_condition_codes_follow_the_flags() {
    // (cond, predicate over (N, Z, C, V)) from the ARM ARM table A8.3.
    type Pred = fn(bool, bool, bool, bool) -> bool;
    let table: [(u32, Pred); 15] = [
        (0, |_, z, _, _| z),
        (1, |_, z, _, _| !z),
        (2, |_, _, c, _| c),
        (3, |_, _, c, _| !c),
        (4, |n, _, _, _| n),
        (5, |n, _, _, _| !n),
        (6, |_, _, _, v| v),
        (7, |_, _, _, v| !v),
        (8, |_, z, c, _| c && !z),
        (9, |_, z, c, _| !c || z),
        (10, |n, _, _, v| n == v),
        (11, |n, _, _, v| n != v),
        (12, |n, z, _, v| !z && n == v),
        (13, |n, z, _, v| z || n != v),
        (14, |_, _, _, _| true),
    ];
    for nibble in 0..16u32 {
        let (n, z, c, v) = (
            nibble & 8 != 0,
            nibble & 4 != 0,
            nibble & 2 != 0,
            nibble & 1 != 0,
        );
        for &(cond, pred) in &table {
            // MOV r0, #1 with the condition field set.
            let insn = (cond << 28) | 0x03A0_0001;
            let mut m = machine(&[insn]);
            m.cpu.set_cpsr(
                0x10 | (u32::from(n) << 31)
                    | (u32::from(z) << 30)
                    | (u32::from(c) << 29)
                    | (u32::from(v) << 28),
            );
            let _ = m.step();
            let expect = u32::from(pred(n, z, c, v));
            assert_eq!(m.cpu.reg(0), expect, "cond {cond} flags NZCV={nibble:04b}");
        }
    }
}

#[test]
fn loads_and_stores_cover_offset_index_and_sign_modes() {
    let n = PROG_LDST.len();
    let mut m = machine(PROG_LDST);
    m.run(n);
    assert_eq!(
        m.cpu.reg(1),
        0x2000,
        "post-indexed base after the last access"
    );
    assert_eq!(m.cpu.reg(4), 0x11, "ldr");
    assert_eq!(m.cpu.reg(5), 0x22, "ldrb");
    assert_eq!(m.cpu.reg(6), 0x11, "ldr post-index");
    assert_eq!(m.cpu.reg(8), 0x80, "ldrsh of a positive halfword");
    assert_eq!(m.cpu.reg(10), 0xFFFF_FFFF, "ldrsb of 0xFF");
    assert_eq!(m.cpu.reg(11), 0x80, "ldrh");
    assert_eq!(m.cpu.reg(7), 0x22, "ldrd second word");
}

#[test]
fn ldm_and_stm_all_four_modes_with_writeback() {
    let n = PROG_LDM.len();
    let mut m = machine(PROG_LDM);
    m.run(n);
    assert_eq!(m.cpu.reg(7), 4, "ldmdb");
    assert_eq!(m.cpu.reg(8), 5);
    assert_eq!(m.cpu.reg(9), 6);
    assert_eq!(m.cpu.reg(10), 4, "ldmda: lowest word at base-4n+4");
    assert_eq!(m.cpu.reg(11), 5);
    assert_eq!(m.cpu.reg(1), 0x2000, "ldmda writeback");
}

#[test]
fn multiplies_and_long_accumulates() {
    let n = PROG_MUL.len();
    let mut m = machine(PROG_MUL);
    m.run(n);
    assert_eq!(m.cpu.reg(0), 42, "mul");
    assert_eq!(m.cpu.reg(3), 84, "mla");
    assert_eq!(m.cpu.reg(4), 42, "mls");
    assert_eq!(m.cpu.reg(6), 43, "umlal low word");
    assert_eq!(
        m.cpu.reg(7),
        0xFFFF_FFFE,
        "umull high word, unchanged by umlal"
    );
    assert_eq!(m.cpu.reg(8), 0xFFFF_FFFB, "smlal low word");
    assert_eq!(m.cpu.reg(9), 0xFFFF_FFFF, "smlal high word (sign of -5)");
    assert_eq!(m.cpu.reg(10), 1);
    assert_eq!(
        m.flags(),
        (false, false, m.flags().2, m.flags().3),
        "muls sets N and Z"
    );
}

#[test]
fn halfword_multiplies_select_halves() {
    let n = PROG_HALFMUL.len();
    let mut m = machine(PROG_HALFMUL);
    m.run(n);
    assert_eq!(m.cpu.reg(0), 0xFFFF_FFFE, "smulbb: -1 * 2");
    // smlabt: bottom(r1) * top(r2) + r2 = -1 * 2 + 0x0002_0002
    assert_eq!(m.cpu.reg(3), 0x0002_0000);
}

#[test]
fn reversal_bit_and_field_instructions() {
    let n = PROG_BITS.len();
    let mut m = machine(PROG_BITS);
    m.run(n);
    assert_eq!(m.cpu.reg(1), 0x7856_3412);
    assert_eq!(m.cpu.reg(2), 0x1234_5678, "rev");
    assert_eq!(m.cpu.reg(3), 0x5678_1234, "rev16");
    assert_eq!(m.cpu.reg(4), 0x0000_1234, "revsh");
    assert_eq!(m.cpu.reg(5), 0x482C_6A1E, "rbit");
    assert_eq!(m.cpu.reg(6), 1, "clz of 0x7...");
    assert_eq!(m.cpu.reg(8), 32, "clz of zero");
    assert_eq!(m.cpu.reg(9), 0x34, "ubfx #8, #8");
    assert_eq!(m.cpu.reg(10), 7, "sbfx #28, #4 of 0111");
    assert_eq!(m.cpu.reg(12), 0xFFFF_F120, "bfi then bfc");
}

#[test]
fn extend_instructions_including_the_accumulate_forms() {
    let n = PROG_EXTEND.len();
    let mut m = machine(PROG_EXTEND);
    m.run(n);
    assert_eq!(m.cpu.reg(2), 0xFFFF_FF80, "sxtb");
    assert_eq!(m.cpu.reg(3), 0x80, "uxtb");
    assert_eq!(m.cpu.reg(4), 0xFFFF_FF80, "sxth");
    assert_eq!(m.cpu.reg(5), 0xFF80, "uxth");
    assert_eq!(m.cpu.reg(6), 0x81, "uxtab: 1 + zero-extended byte");
    assert_eq!(
        m.cpu.reg(8),
        0xFFFF_FF81,
        "sxtah: 1 + sign-extended halfword"
    );
}

#[test]
fn saturating_instructions_set_q() {
    let n = PROG_SAT.len();
    let mut m = machine(PROG_SAT);
    m.run(n);
    assert_eq!(m.cpu.reg(3), 0x7F, "ssat #8 of 300 saturates to 127");
    assert_eq!(m.cpu.reg(5), 0, "usat #8 of -5 saturates to 0");
    assert_eq!(m.cpu.reg(7), 0x7FFF_FFFF, "qadd saturates");
    assert_ne!(m.cpu.cpsr() & (1 << 27), 0, "Q is set");
}

#[test]
fn parallel_add_subtract_sets_ge_for_the_plain_forms_only() {
    let n = PROG_PAR.len();
    let mut m = machine(PROG_PAR);
    m.run(n);
    // r1 = bytes [02 01 04 03], r2 = all 01.
    assert_eq!(m.cpu.reg(3), 0x0203_0001, "uqsub8 (bytes 1,0,3,2)");
    assert_eq!(
        m.cpu.reg(4),
        0x0405_0203,
        "sadd8 wraps to bytes 3,2,5,4 (lane 0 is 0x03)"
    );
    assert_eq!(m.cpu.reg(5), 0x0405_0203, "sadd16 halves 0x0203 and 0x0405");
    assert_eq!(m.cpu.reg(6), 0x0202_0101, "uhadd16 halves the sums");
    assert_eq!((m.cpu.cpsr() >> 16) & 0xF, 0xF, "GE from the last plain op");
}

#[test]
fn pack_halfword_instructions() {
    let n = PROG_PKH.len();
    let mut m = machine(PROG_PKH);
    m.run(n);
    assert_eq!(m.cpu.reg(3), 0xF01A_5678, "pkhbt lsl #4");
    assert_eq!(m.cpu.reg(4), 0x1234_1ABC, "pkhtb asr #4");
}

#[test]
fn msr_and_mrs_read_and_write_the_flags_and_the_mode() {
    let n = PROG_STATUS.len();
    let mut m = machine(PROG_STATUS);
    m.run(n);
    assert_eq!(
        m.cpu.reg(1),
        0xF000_00D3,
        "flags written by msr cpsr_f, read back"
    );
    assert_eq!(
        m.cpu.reg(2),
        0xF000_0010,
        "msr cpsr_c, #0x10 drops to user mode, unmasks"
    );
    assert_eq!(m.cpu.mode(), Mode::User);
}

#[test]
fn banked_stack_pointers_are_per_mode() {
    let n = PROG_BANK.len();
    let mut m = machine(PROG_BANK);
    m.run(n);
    assert_eq!(m.cpu.mode(), Mode::Supervisor);
    assert_eq!(
        m.cpu.reg(13),
        0x2000,
        "svc sp survives a trip through IRQ mode"
    );
}

#[test]
fn msr_spsr_and_mrs_spsr() {
    let n = PROG_MSR_SPSR.len();
    let mut m = machine(PROG_MSR_SPSR);
    m.run(n);
    assert_eq!(m.cpu.reg(0), 0x13);
    assert_eq!(m.cpu.spsr(Mode::Supervisor), 0x13);
}

#[test]
fn compares_set_flags_without_writing_and_conditional_moves_follow_them() {
    let n = PROG_CMPS.len();
    let mut m = machine(PROG_CMPS);
    m.run(n);
    assert_eq!(m.cpu.reg(1), 1, "moveq ran, movne did not");
    assert_eq!(
        m.flags(),
        (false, false, false, false),
        "cmn 5, -1 gives 6: Z clear"
    );
}

#[test]
fn svc_takes_the_exception_and_records_the_return() {
    let mut m = machine(PROG_SVC_TRAP);
    m.run(3);
    assert_eq!(m.event(), Some(Event::Svc(0x42)));
    assert_eq!(m.cpu.mode(), Mode::Supervisor);
    assert_eq!(m.cpu.pc(), 0x2000 + 8, "svc vector at VBAR+8");
    assert_eq!(m.cpu.reg(14), CODE + 16, "LR is the instruction after svc");
    assert_eq!(
        m.cpu.spsr(Mode::Supervisor) & 0x1F,
        0x10,
        "SPSR holds the user-mode CPSR"
    );
    assert_ne!(m.cpu.cpsr() & 0x80, 0, "IRQ masked on entry");
}

#[test]
fn undefined_instruction_takes_the_undef_exception() {
    let mut m = machine(PROG_UNDEF);
    assert_eq!(m.event(), Some(Event::Undefined));
    assert_eq!(m.cpu.mode(), Mode::Undefined);
    assert_eq!(m.cpu.reg(14), CODE + 4);
    assert_eq!(m.cpu.pc(), 4, "undef vector at VBAR+4 (VBAR reset 0)");
}

#[test]
fn data_abort_records_the_faulting_address_and_status() {
    let mut m = machine(PROG_DABORT);
    m.run(2);
    assert_eq!(m.event(), Some(Event::DataAbort { addr: 0x9000_0000 }));
    assert_eq!(m.cpu.mode(), Mode::Abort);
    assert_eq!(m.cpu.reg(14), CODE + 8 + 8, "LR = faulting instruction + 8");
    assert_eq!(m.cpu.pc(), 0x10, "data abort vector at VBAR+0x10");
    assert_eq!(m.cpu.cp15(0, 6, 0, 0), Some(0x9000_0000), "DFAR");
    assert_eq!(
        m.cpu.cp15(0, 5, 0, 0),
        Some(0x08),
        "DFSR: synchronous external abort"
    );
}

#[test]
fn prefetch_abort_on_an_unmapped_fetch() {
    let mut m = machine(PROG_PABORT);
    m.run(3);
    assert_eq!(m.event(), Some(Event::PrefetchAbort { addr: 0x9000_0000 }));
    assert_eq!(m.cpu.mode(), Mode::Abort);
    assert_eq!(m.cpu.reg(14), 0x9000_0004, "LR = faulting address + 4");
    assert_eq!(m.cpu.pc(), 0x0C, "prefetch abort vector at VBAR+0xC");
    assert_eq!(m.cpu.cp15(0, 5, 0, 1), Some(0x08), "IFSR");
}

#[test]
fn irq_is_taken_between_instructions_and_returns_with_subs_pc_lr_4() {
    let mut m = machine(PROG_IRQ);
    m.bus.load(0x2018, &0xE25E_F004u32.to_le_bytes()); // subs pc, lr, #4 at the IRQ vector
    m.run(4); // movw, mcr (VBAR), msr (user, unmasked), mov r0, #1
    assert_eq!(m.cpu.pc(), CODE + 16);
    m.cpu.irq(true);
    assert_eq!(m.event(), Some(Event::Irq));
    assert_eq!(m.cpu.mode(), Mode::Irq);
    assert_eq!(m.cpu.reg(14), CODE + 16 + 4, "LR = next instruction + 4");
    assert_eq!(m.cpu.pc(), 0x2018, "IRQ vector at VBAR+0x18");
    assert_eq!(m.cpu.spsr(Mode::Irq) & 0x1F, 0x10, "SPSR is the user CPSR");
    m.cpu.irq(false);
    m.run(1); // the handler: subs pc, lr, #4
    assert_eq!(m.cpu.pc(), CODE + 16, "back at the interrupted instruction");
    assert_eq!(m.cpu.mode(), Mode::User, "CPSR restored from SPSR");
    assert_eq!(m.cpu.cpsr() & 0x80, 0, "IRQ unmasked again");
}

#[test]
fn fiq_masks_fiq_and_irq_and_saves_the_return() {
    let mut m = machine(&[0xE3A0_0001]); // mov r0, #1
    m.cpu.set_cpsr(0x10);
    m.cpu.fiq(true);
    assert_eq!(m.event(), Some(Event::Fiq));
    assert_eq!(m.cpu.mode(), Mode::Fiq);
    assert_ne!(m.cpu.cpsr() & 0x40, 0, "F set");
    assert_ne!(m.cpu.cpsr() & 0x80, 0, "I set");
    assert_eq!(m.cpu.reg(14), CODE + 4);
    assert_eq!(m.cpu.pc(), 0x1C, "FIQ vector at VBAR+0x1C");
}

#[test]
fn wfi_waits_then_resumes_after_the_wakeup_even_when_masked() {
    let mut m = machine(PROG_WFI);
    assert_eq!(m.event(), Some(Event::Wfi));
    assert!(m.cpu.is_waiting());
    assert_eq!(m.event(), Some(Event::Idle), "no interrupt yet");
    assert_eq!(
        m.cpu.pc(),
        CODE + 4,
        "the core does not advance while waiting"
    );
    m.cpu.irq(true); // I is masked after reset: not taken, but the wait ends
    m.run(1);
    assert_eq!(m.cpu.reg(0), 9);
    assert!(!m.cpu.is_waiting());
}

#[test]
fn sev_wakes_wfe_and_the_event_register_is_consumed() {
    let mut m = machine(PROG_WFE);
    assert_eq!(m.event(), Some(Event::Sev));
    assert_eq!(m.event(), Some(Event::Wfe), "event register set: no wait");
    assert!(!m.cpu.is_waiting());
    assert_eq!(m.event(), Some(Event::Wfe));
    assert!(m.cpu.is_waiting());
    assert_eq!(m.event(), Some(Event::Idle));
    m.cpu.send_event();
    m.run(1);
    assert!(!m.cpu.is_waiting());
}

#[test]
fn cp15_identification_control_and_maintenance_registers() {
    let n = PROG_CP15.len();
    let mut m = machine(PROG_CP15);
    m.run(n);
    assert_eq!(m.cpu.reg(0), 0x413F_C090, "MIDR of a Cortex-A9 r3p0");
    assert_eq!(m.cpu.reg(1), 0x8000_0000, "MPIDR core 0");
    assert_eq!(m.cpu.reg(3), 0x1234, "ACTLR round trip");
    let other = Cpu::new(1);
    assert_eq!(other.cp15(0, 0, 0, 5), Some(0x8000_0001), "MPIDR of core 1");
}

#[test]
fn enabling_the_mmu_with_no_tables_takes_a_prefetch_abort() {
    // SCTLR.M is set with TTBR0 = 0, so the first-level walk reads address 0, which this
    // bus does not map: a synchronous external abort on a first-level walk (IFSR 0x0C).
    // The MMU itself is tested in `tests/mmu.rs`.
    let mut m = machine(PROG_MMU);
    m.run(2);
    assert_eq!(m.event(), Some(Event::PrefetchAbort { addr: CODE + 8 }));
    assert_eq!(
        m.cpu.cp15(0, 5, 0, 1),
        Some(0x0C),
        "IFSR: walk, first level"
    );
    assert_eq!(m.cpu.cp15(0, 6, 0, 2), Some(CODE + 8), "IFAR");
}

#[test]
fn exclusive_monitor_succeeds_once_and_fails_after_a_clear() {
    let n = PROG_EXCL.len();
    let mut m = machine(PROG_EXCL);
    m.run(n);
    assert_eq!(m.cpu.reg(3), 0, "first strex after ldrex succeeds");
    assert_eq!(m.cpu.reg(5), 1, "second strex fails: monitor was cleared");
    assert_eq!(m.cpu.reg(6), 9, "ldrex read back the stored value");
    assert_eq!(m.cpu.reg(7), 1, "strex after clrex fails");
}

#[test]
fn half_precision_vcvt_is_reported_unimplemented_and_nothing_changes() {
    // VFP proper is implemented (see vfp_isa.rs). With FPEXC.EN clear (reset state) the access
    // check comes first, so VCVTB is UNDEFINED; once enabled it is reported Unimplemented.
    let mut m = machine(&[0xEEB2_1A40]); // vcvtb.f32.f16 s2, s0
    match m.step() {
        StepResult::Executed {
            event: Some(cpu_a9::Event::Undefined),
            ..
        } => {}
        other => panic!("expected Undefined while VFP is disabled, got {other:?}"),
    }
}

#[test]
fn bx_to_an_odd_address_enters_thumb_which_is_reported() {
    let mut m = machine(PROG_BXODD);
    m.run(2);
    assert_ne!(m.cpu.cpsr() & 0x20, 0, "T set");
    match m.step() {
        StepResult::Unimplemented { pc, .. } => assert_eq!(pc, 0x2000),
        other => panic!("expected Unimplemented, got {other:?}"),
    }
}

#[test]
fn unallocated_unconditional_encodings_are_reported_not_guessed() {
    // 0xF0000000 is in the unconditional space and not one this core models.
    let mut m = machine(&[0xF000_0000]);
    assert!(matches!(
        m.step(),
        StepResult::Unimplemented { pc: CODE, .. }
    ));
}

#[test]
fn ldr_to_pc_branches_to_the_loaded_address() {
    let mut m = machine(PROG_LDRPC);
    m.cpu.set_reg(13, 0x2800);
    m.run(PROG_LDRPC.len());
    assert_eq!(m.cpu.pc(), 0x1020);
}

#[test]
fn a_single_instruction_takes_one_cycle() {
    let mut m = machine(&[0xE3A0_0001]);
    match m.step() {
        StepResult::Executed { pc, cycles, event } => {
            assert_eq!(pc, CODE);
            assert_eq!(cycles, 1);
            assert_eq!(event, None);
        }
        other => panic!("{other:?}"),
    }
}
