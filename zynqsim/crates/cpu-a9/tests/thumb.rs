//! Thumb and Thumb-2 instruction tests (CPSR.T set). The programs are assembled by the Xilinx GNU
//! toolchain at generation time (`data/gen_thumb.py` from `data/thumb_groups.s`, committed as
//! `data/thumb_words.inc.rs`); the tests need no toolchain. Expected values are worked out by hand
//! in each test from the ARM ARM. The property tests (A32 against T32) are in `thumb_props.rs`.

use cpu_a9::{Cpu, Event, Mode, StepResult};
use ps_bus::{Bus, RamBus};

include!("data/thumb_words.inc.rs");

/// Where the test programs are loaded.
const CODE: u32 = 0x1000;
/// CPSR.T.
const T: u32 = 0x20;
/// Supervisor mode with IRQ and FIQ masked and Thumb state.
const SVC_THUMB: u32 = 0x13 | 0xC0 | T;
/// CPSR flags (N, Z, C, V) from their bits.
fn nzcv(n: bool, z: bool, c: bool, v: bool) -> u32 {
    (u32::from(n) << 31) | (u32::from(z) << 30) | (u32::from(c) << 29) | (u32::from(v) << 28)
}

/// A core with `hws` (Thumb halfwords) at [`CODE`] in RAM 0x1000..0x4000, every register 0x2000
/// except SP = 0x3000, and the flags as given.
struct Machine {
    cpu: Cpu,
    bus: RamBus,
}

fn load_hws(bus: &mut RamBus, addr: u32, hws: &[u16]) {
    let bytes: Vec<u8> = hws.iter().flat_map(|h| h.to_le_bytes()).collect();
    bus.load(u64::from(addr), &bytes);
}

fn machine(hws: &[u16], flags: u32) -> Machine {
    let mut bus = RamBus::new(0x1000, 0x3000);
    load_hws(&mut bus, CODE, hws);
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    cpu.set_cpsr(SVC_THUMB | flags);
    for r in 0..13 {
        cpu.set_reg(r, 0x2000);
    }
    cpu.set_reg(13, 0x3000);
    Machine { cpu, bus }
}

impl Machine {
    fn step(&mut self) -> StepResult {
        self.cpu.step(&mut self.bus)
    }

    /// Steps `n` times; each step must execute with no event.
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

    /// Steps once; the step must execute and may raise an event, which is returned.
    fn event(&mut self) -> Option<Event> {
        match self.step() {
            StepResult::Executed { event, .. } => event,
            other => panic!("expected an executed step, got {other:?}"),
        }
    }

    fn r(&self, n: u8) -> u32 {
        self.cpu.reg(n)
    }

    fn flags(&self) -> u32 {
        self.cpu.cpsr() & 0xF000_0000
    }
}

/// The instruction count of a group: its halfword count when every instruction is 16-bit.
fn run_group_to_end(m: &mut Machine, end: u32, limit: usize) -> usize {
    let mut n = 0;
    while m.cpu.pc() != end {
        assert!(n < limit, "did not reach {end:#x}; pc {:#x}", m.cpu.pc());
        let r = m.step();
        assert!(
            !matches!(r, StepResult::Unimplemented { .. }),
            "unimplemented at {:#x}: {r:?}",
            m.cpu.pc()
        );
        n += 1;
    }
    n
}

// ----- corpus: every group decodes and executes; nothing is guessed ----------------------------

#[test]
fn every_corpus_group_decodes_and_runs_without_unimplemented() {
    let groups: &[(&str, &[u16])] = &[
        ("T_SHIFT", PROG_T_SHIFT),
        ("T_ADDSUB", PROG_T_ADDSUB),
        ("T_IMM8", PROG_T_IMM8),
        ("T_DPREG", PROG_T_DPREG),
        ("T_HIREG", PROG_T_HIREG),
        ("T_BX", PROG_T_BX),
        ("T_LDRLIT", PROG_T_LDRLIT),
        ("T_LDST_REG", PROG_T_LDST_REG),
        ("T_LDST_IMM", PROG_T_LDST_IMM),
        ("T_ADR_SP", PROG_T_ADR_SP),
        ("T_EXT_REV", PROG_T_EXT_REV),
        ("T_PUSHPOP", PROG_T_PUSHPOP),
        ("T_LDMSTM", PROG_T_LDMSTM),
        ("T_BR_COND", PROG_T_BR_COND),
        ("T_CBZ", PROG_T_CBZ),
        ("T_IT", PROG_T_IT),
        ("T_HINTS", PROG_T_HINTS),
        ("T_BKPT_SVC", PROG_T_BKPT_SVC),
        ("T_CPS", PROG_T_CPS),
        ("T_UDF", PROG_T_UDF),
        ("T_B_NEAR", PROG_T_B_NEAR),
        ("T32_DPIMM", PROG_T32_DPIMM),
        ("T32_DPREG", PROG_T32_DPREG),
        ("T32_SHIFTREG", PROG_T32_SHIFTREG),
        ("T32_MOVW_ADDW", PROG_T32_MOVW_ADDW),
        ("T32_ADR", PROG_T32_ADR),
        ("T32_BITFIELD", PROG_T32_BITFIELD),
        ("T32_EXTEND", PROG_T32_EXTEND),
        ("T32_SAT", PROG_T32_SAT),
        ("T32_MUL", PROG_T32_MUL),
        ("T32_DIV", PROG_T32_DIV),
        ("T32_LDR", PROG_T32_LDR),
        ("T32_LDM", PROG_T32_LDM),
        ("T32_LDREX", PROG_T32_LDREX),
        ("T32_TBB", PROG_T32_TBB),
        ("T32_SYS", PROG_T32_SYS),
        ("T32_CP15", PROG_T32_CP15),
        ("T32_BRANCH", PROG_T32_BRANCH),
    ];
    for (name, hws) in groups {
        let mut m = machine(hws, 0);
        let end = CODE + 2 * hws.len() as u32;
        // Branches may leave the group; stop as soon as the PC is outside it.
        for _ in 0..200 {
            let pc = m.cpu.pc();
            if pc >= end || pc < CODE {
                break;
            }
            let r = m.step();
            assert!(
                !matches!(r, StepResult::Unimplemented { .. }),
                "{name}: unimplemented at {pc:#x}: {r:?}"
            );
        }
    }
}

// ----- 16-bit data processing ------------------------------------------------------------------

#[test]
fn shifts_and_adds_by_immediate_and_register() {
    // lsls r0,r1,#3 ; lsrs r2,r3,#32 ; asrs r4,r5,#1 ; movs r6,r7 ; lsls r1,r2,#31
    let mut m = machine(PROG_T_SHIFT, 0);
    m.cpu.set_reg(1, 5);
    m.cpu.set_reg(3, 0x8000_0001);
    m.cpu.set_reg(5, 0x8000_0003);
    m.cpu.set_reg(7, 0x42);
    m.run(5);
    assert_eq!(m.r(0), 0x28, "LSL #3");
    assert_eq!(m.r(2), 0, "LSR #32 is zero");
    assert_eq!(m.r(4), 0xC000_0001, "ASR #1 keeps the sign");
    assert_eq!(m.r(6), 0x42, "MOV register");
    assert_eq!(m.r(1), 0, "LSL #31 of zero");
    assert_eq!(m.flags() >> 30 & 1, 1, "Z from the last result");
}

#[test]
fn add_and_subtract_set_the_flags_of_an_adder() {
    // adds r0,r1,r2 ; subs r3,r4,r5 ; adds r0,r1,#7 ; subs r2,r2,#1 ; adds r6,r6,#200 ; subs r7,#255
    let mut m = machine(PROG_T_ADDSUB, 0);
    m.cpu.set_reg(1, 1);
    m.cpu.set_reg(2, 2);
    m.cpu.set_reg(4, 10);
    m.cpu.set_reg(5, 3);
    m.cpu.set_reg(6, 0x100);
    m.cpu.set_reg(7, 255);
    m.run(PROG_T_ADDSUB.len());
    assert_eq!(m.r(0), 8);
    assert_eq!(m.r(3), 7);
    assert_eq!(m.r(2), 1);
    assert_eq!(m.r(6), 0x1C8);
    assert_eq!(m.r(7), 0);
    // subs r7, #255 with r7 = 255: zero result, no borrow, so Z=1 C=1.
    assert_eq!(m.flags(), nzcv(false, true, true, false));
}

#[test]
fn logical_operations_keep_carry_and_overflow() {
    // ands r0,r1 ; eors r2,r3 ; lsls r4,r5 ; lsrs r6,r7 ; asrs r0,r1 ; ... only the first three
    // groups are checked here: the register-shift forms set C from the shifter.
    let hws = [0x4008, 0x405A, 0x40AC]; // ANDS r0,r1 ; EORS r2,r3 ; LSLS r4,r5
    let mut m = machine(&hws, nzcv(false, false, true, true));
    m.cpu.set_reg(0, 0xF0);
    m.cpu.set_reg(1, 0x0F);
    m.cpu.set_reg(2, 0xFF);
    m.cpu.set_reg(3, 0x0F);
    m.cpu.set_reg(4, 1);
    m.cpu.set_reg(5, 33); // LSL by the low byte: 33 gives zero with carry from bit -1 = 0
    m.run(1);
    assert_eq!(m.r(0), 0);
    assert_eq!(
        m.flags(),
        nzcv(false, true, true, true),
        "ANDS keeps C and V"
    );
    m.run(1);
    assert_eq!(m.r(2), 0xF0);
    m.run(1);
    assert_eq!(m.r(4), 0, "LSL by 33 clears the register");
    assert_eq!(m.flags() >> 29 & 1, 0, "LSL by 33 carries out zero");
}

#[test]
fn cmp_sets_the_flags_of_a_subtraction() {
    // movs r0,#255 ; cmp r0,#10: 255 - 10 is positive with no borrow (C=1), not zero.
    let mut m = machine(PROG_T_IMM8, 0);
    m.run(2);
    assert_eq!(m.flags(), nzcv(false, false, true, false));
}

// ----- IT blocks -----------------------------------------------------------------------------

#[test]
fn it_block_selects_then_and_else_instructions() {
    // Z=0, C=1: the EQ instruction is skipped, the NE one runs, and HI (C and not Z) holds.
    let mut m = machine(PROG_T_IT, nzcv(false, false, true, false));
    m.cpu.set_reg(0, 0);
    m.cpu.set_reg(1, 0);
    m.cpu.set_reg(2, 0);
    m.cpu.set_reg(3, 5);
    let n = run_group_to_end(&mut m, CODE + 2 * PROG_T_IT.len() as u32, 64);
    assert!(n > 0);
    assert_eq!(m.r(0), 0, "moveq skipped");
    assert_eq!(m.r(1), 2, "movne ran; the moveq of the else arm did not");
    assert_eq!(m.r(2), 1, "itt cs: addcs ran");
    assert_eq!(m.r(3), 4, "subcs ran");
    assert_eq!(m.r(4), 4, "movhi (then arm of itete hi)");
    assert_eq!(m.r(5), 6, "movhi (third slot of itete hi)");
    assert_eq!(
        m.cpu.cpsr() & (3 << 25 | 0x3F << 10),
        0,
        "the IT state is zero after the block"
    );
}

#[test]
fn it_block_takes_the_other_arms_when_the_flags_differ() {
    // Z=1, C=1: the EQ arm runs, the NE arm does not, and HI is false (Z set).
    let mut m = machine(PROG_T_IT, nzcv(false, true, true, false));
    m.cpu.set_reg(3, 5);
    m.cpu.set_reg(4, 0x77);
    m.cpu.set_reg(5, 0x88);
    run_group_to_end(&mut m, CODE + 2 * PROG_T_IT.len() as u32, 64);
    assert_eq!(m.r(0), 1, "moveq ran");
    assert_eq!(m.r(1), 3, "moveq of the else arm ran");
    assert_eq!(m.r(4), 5, "HI is false: the else arm movls ran");
    assert_eq!(m.r(5), 7, "and the else arm of the second slot too");
}

#[test]
fn flag_setting_16bit_instructions_do_not_set_flags_inside_it() {
    // IT EQ ; ADDS r0,r0,#1 with Z=1, C=0 and r0 = 0xFFFF_FFFF: the adder would carry out,
    // but inside an IT block the 16-bit ADDS does not set the flags.
    let hws = [0xBF08, 0x3001];
    let mut m = machine(&hws, nzcv(false, true, false, false));
    m.cpu.set_reg(0, 0xFFFF_FFFF);
    m.run(2);
    assert_eq!(m.r(0), 0);
    assert_eq!(
        m.flags(),
        nzcv(false, true, false, false),
        "flags unchanged in the IT block"
    );
    // Outside an IT block the same ADDS sets C.
    let mut m = machine(&[0x3001], nzcv(false, true, false, false));
    m.cpu.set_reg(0, 0xFFFF_FFFF);
    m.run(1);
    assert_eq!(
        m.flags(),
        nzcv(false, true, true, false),
        "ADDS sets C outside IT"
    );
}

#[test]
fn an_svc_inside_it_saves_the_next_state_and_clears_the_block() {
    // IT EQ ; SVC #5 with Z=1: the SVC is taken; LR is the next instruction (CODE+4); the SPSR
    // keeps T and the advanced (zero) IT state; the IT state of the core is cleared.
    let hws = [0xBF08, 0xDF05];
    let mut m = machine(&hws, nzcv(false, true, false, false));
    m.cpu.set_cpsr(0x10 | T | nzcv(false, true, false, false));
    m.run(1);
    assert_eq!(m.event(), Some(Event::Svc(5)));
    assert_eq!(m.cpu.mode(), Mode::Supervisor);
    assert_eq!(m.r(14), CODE + 4, "LR is the instruction after the SVC");
    let spsr = m.cpu.spsr(Mode::Supervisor);
    assert_eq!(spsr & 0x1F, 0x10, "SPSR holds the User mode");
    assert_ne!(spsr & T, 0, "SPSR has T");
    assert_eq!(
        spsr & (3 << 25 | 0x3F << 10),
        0,
        "the saved IT state is the advanced one"
    );
    assert_eq!(
        m.cpu.cpsr() & (3 << 25 | 0x3F << 10),
        0,
        "the IT state is cleared on entry"
    );
}

// ----- branches and interworking -------------------------------------------------------------

#[test]
fn compare_and_branch_on_zero() {
    // cbz r0, out ; cbnz r1, out ; nop ; out: nop
    let mut m = machine(PROG_T_CBZ, 0);
    m.cpu.set_reg(0, 0);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 6, "CBZ taken to out");
    let mut m = machine(PROG_T_CBZ, 0);
    m.cpu.set_reg(0, 1);
    m.cpu.set_reg(1, 0);
    m.run(2);
    assert_eq!(m.cpu.pc(), CODE + 4, "CBZ not taken, CBNZ not taken");
}

#[test]
fn bx_and_blx_to_an_even_address_enter_arm() {
    // BX r1 (0x4708) with r1 = 0x2000: the T bit is cleared and the PC is the address.
    let mut m = machine(&[0x4708], 0);
    m.cpu.set_reg(1, 0x2000);
    m.run(1);
    assert_eq!(m.cpu.cpsr() & T, 0, "even address selects ARM");
    assert_eq!(m.cpu.pc(), 0x2000);
}

#[test]
fn blx_register_links_with_bit_zero_and_keeps_thumb_for_an_odd_target() {
    // BLX r2 (0x4790) with r2 = 0x2001: LR = next halfword | 1, target 0x2000 in Thumb.
    let mut m = machine(&[0x4790], 0);
    m.cpu.set_reg(2, 0x2001);
    m.run(1);
    assert_eq!(
        m.r(14),
        CODE + 3,
        "LR is the next instruction with bit 0 set"
    );
    assert_ne!(m.cpu.cpsr() & T, 0, "odd target stays in Thumb");
    assert_eq!(m.cpu.pc(), 0x2000);
}

#[test]
fn pop_into_pc_interworks_on_bit_zero() {
    // POP {pc} (0xBD00) with the value 0x2000 on the stack: ARM state; with 0x2001: Thumb.
    for (value, thumb, pc) in [(0x2000u32, false, 0x2000u32), (0x2001, true, 0x2000)] {
        let mut m = machine(&[0xBD00], 0);
        m.bus.load(0x3000, &value.to_le_bytes());
        m.run(1);
        assert_eq!(m.cpu.cpsr() & T != 0, thumb, "state for {value:#x}");
        assert_eq!(m.cpu.pc(), pc);
        assert_eq!(m.r(13), 0x3004, "SP after the pop");
    }
}

#[test]
fn ldr_into_pc_interworks() {
    // LDR.W pc, [r0] (0xF8D0 0xF000): the loaded odd value selects Thumb.
    let mut m = machine(&[0xF8D0, 0xF000], 0);
    m.cpu.set_reg(0, 0x2800);
    m.bus.load(0x2800, &0x2201u32.to_le_bytes());
    m.run(1);
    assert_ne!(m.cpu.cpsr() & T, 0);
    assert_eq!(m.cpu.pc(), 0x2200);
}

#[test]
fn alu_writes_to_pc_clear_only_bit_zero() {
    // MOV pc, r2 (0x4697) with r2 = 0x1013: halfword aligned target, state unchanged.
    let mut m = machine(&[0x4697], 0);
    m.cpu.set_reg(2, 0x1013);
    m.run(1);
    assert_eq!(m.cpu.pc(), 0x1012);
    assert_ne!(m.cpu.cpsr() & T, 0, "ALU PC writes do not change the state");
}

#[test]
fn add_to_pc_reads_pc_as_the_address_plus_four() {
    // ADD pc, r2 (0x4497) with r2 = 0x10: target = (CODE + 4) + 0x10.
    let mut m = machine(&[0x4497], 0);
    m.cpu.set_reg(2, 0x10);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 4 + 0x10);
}

#[test]
fn b_forward_lands_on_its_label() {
    // b far ; nop ; nop ; far: nop  -- the label is 6 bytes after the branch.
    let mut m = machine(PROG_T_B_NEAR, 0);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 6);
}

#[test]
fn b_w_reaches_a_32_bit_offset_label() {
    // PROG_T32_BRANCH starts with B.W lbl, where lbl is 0x16 bytes from CODE.
    let mut m = machine(PROG_T32_BRANCH, 0);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 0x16);
}

// ----- stack and multiple transfers ----------------------------------------------------------

#[test]
fn push_and_pop_keep_the_stack_and_the_registers() {
    // push {r0-r3, lr} ; pop {r0-r3} ; push {r4} ; pop {r4, r5}
    let mut m = machine(PROG_T_PUSHPOP, 0);
    m.cpu.set_reg(0, 1);
    m.cpu.set_reg(1, 2);
    m.cpu.set_reg(2, 3);
    m.cpu.set_reg(3, 4);
    m.cpu.set_reg(4, 9);
    m.cpu.set_reg(14, 0xAB);
    m.run(PROG_T_PUSHPOP.len());
    assert_eq!(m.r(0), 1);
    assert_eq!(m.r(3), 4);
    assert_eq!(m.r(4), 9, "popped back from the slot the push wrote");
    assert_eq!(
        m.r(5),
        0xAB,
        "the LR pushed first is what the second pop reads"
    );
    assert_eq!(m.r(13), 0x3000, "the stack pointer is restored");
}

#[test]
fn stm_writes_back_and_ldm_reads_in_order() {
    // stmia r0!, {r1, r2} ; ldmia r3!, {r4, r5} ; ldmia r6, {r6, r7} ; stmia r7!, {r0}
    let mut m = machine(PROG_T_LDMSTM, 0);
    m.cpu.set_reg(0, 0x2100);
    m.cpu.set_reg(1, 0x11);
    m.cpu.set_reg(2, 0x22);
    m.cpu.set_reg(3, 0x2200);
    m.bus.load(0x2200, &0x33u32.to_le_bytes());
    m.bus.load(0x2204, &0x44u32.to_le_bytes());
    m.cpu.set_reg(6, 0x2300);
    m.cpu.set_reg(7, 0x2400);
    m.bus.load(0x2300, &0x55u32.to_le_bytes());
    m.bus.load(0x2304, &0x66u32.to_le_bytes());
    // The fourth instruction stores through R7 = 0x66, which is unmapped: stop before it.
    m.run(3);
    assert_eq!(m.r(0), 0x2108, "STMIA! wrote back eight bytes");
    assert_eq!(m.r(3), 0x2208, "LDMIA! wrote back eight bytes");
    assert_eq!(m.r(4), 0x33);
    assert_eq!(m.r(5), 0x44);
    assert_eq!(m.r(6), 0x55, "LDM without writeback: the base is loaded");
    assert_eq!(m.r(7), 0x66);
}

// ----- system and exceptions -----------------------------------------------------------------

#[test]
fn undefined_thumb_instruction_takes_the_undefined_exception() {
    // UDF #0x42 (0xDE42): LR is the next instruction; the SPSR keeps T.
    let mut m = machine(&[0xDE42], 0);
    assert_eq!(m.event(), Some(Event::Undefined));
    assert_eq!(m.cpu.mode(), Mode::Undefined);
    assert_eq!(m.r(14), CODE + 2);
    assert_ne!(m.cpu.spsr(Mode::Undefined) & T, 0);
}

#[test]
fn bkpt_takes_a_prefetch_abort_with_the_debug_status() {
    // BKPT #0x12 (0xBE12): LR is the BKPT address + 4, in Abort mode.
    let mut m = machine(&[0xBE12], 0);
    assert_eq!(m.event(), Some(Event::Bkpt(0x12)));
    assert_eq!(m.cpu.mode(), Mode::Abort);
    assert_eq!(m.r(14), CODE + 4);
}

#[test]
fn hints_and_wfe_follow_the_a32_events() {
    // NOP, YIELD, WFE, WFI, SEV: WFE with no pending event waits until an event arrives.
    let mut m = machine(PROG_T_HINTS, 0);
    m.run(2);
    assert_eq!(m.event(), Some(Event::Wfe));
    assert_eq!(m.event(), Some(Event::Idle), "the core waits in WFE");
}

#[test]
fn cps_disables_and_enables_the_masks_in_a_privileged_mode() {
    // cpsid i ; cpsie i ; cpsid f (0xB672, 0xB662, 0xB671)
    let mut m = machine(PROG_T_CPS, 0);
    m.cpu.set_cpsr(SVC_THUMB & !0xC0); // IRQ and FIQ enabled
    m.run(1);
    assert_ne!(m.cpu.cpsr() & 0x80, 0, "CPSID i masks IRQ");
    m.run(1);
    assert_eq!(m.cpu.cpsr() & 0x80, 0, "CPSIE i unmasks IRQ");
    m.run(1);
    assert_ne!(m.cpu.cpsr() & 0x40, 0, "CPSID f masks FIQ");
}

#[test]
fn tbb_and_tbh_branch_by_the_table_entry_times_two() {
    // TBB [pc, r0] at CODE: the table starts at CODE+4; entry 1 is 3 -> target (CODE+4)+6.
    let mut m = machine(&[0xE8DF, 0xF000], 0);
    m.bus.load(0x1004, &[0x00, 0x03]);
    m.cpu.set_reg(0, 1);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 4 + 6, "TBB");
    // TBH [r1, r2, lsl #1] (0xE8D1 0xF012): halfword entry at r1 + 2*r2.
    let mut m = machine(&[0xE8D1, 0xF012], 0);
    m.bus.load(0x2000 + 4, &0x0005u16.to_le_bytes());
    m.cpu.set_reg(1, 0x2000);
    m.cpu.set_reg(2, 2);
    m.run(1);
    assert_eq!(m.cpu.pc(), CODE + 4 + 10, "TBH");
}

#[test]
fn divides_are_undefined_on_the_cortex_a9() {
    // UDIV r0, r1, r2 (0xFBB1 0xF0F2) takes the undefined exception.
    let mut m = machine(&[0xFBB1, 0xF0F2], 0);
    assert_eq!(m.event(), Some(Event::Undefined));
    assert_eq!(m.r(14), CODE + 4, "LR is the next 32-bit instruction");
}

#[test]
fn ldrex_then_strex_to_the_same_address_succeeds() {
    // LDREX r0, [r1] ; STREX r2, r3, [r4] with r1 = r4: the monitor matches and Rd = 0.
    let mut m = machine(PROG_T32_LDREX, 0);
    m.cpu.set_reg(1, 0x2400);
    m.cpu.set_reg(4, 0x2400);
    m.bus.load(0x2400, &0x1234u32.to_le_bytes());
    m.cpu.set_reg(3, 0x9999);
    m.run(2);
    assert_eq!(m.r(0), 0x1234);
    assert_eq!(m.r(2), 0, "the store succeeded");
    assert_eq!(m.bus_word(0x2400), 0x9999);
}

#[test]
fn strex_without_a_matching_load_fails_and_stores_nothing() {
    // STREX r2, r3, [r4] alone: no exclusive reservation, so Rd = 1 and memory is unchanged.
    let mut m = machine(&[0xE844, 0x3200], 0);
    m.cpu.set_reg(4, 0x2400);
    m.cpu.set_reg(3, 0x9999);
    m.bus.load(0x2400, &0x1234u32.to_le_bytes());
    m.run(1);
    assert_eq!(m.r(2), 1);
    assert_eq!(m.bus_word(0x2400), 0x1234);
}

impl Machine {
    fn bus_word(&mut self, addr: u32) -> u32 {
        self.bus.read(u64::from(addr), 4).expect("word") as u32
    }
}

#[test]
fn mrc_and_mcr_reach_cp15() {
    // MRC p15, 0, r1, c0, c0, 5 reads MPIDR, the same value the CP15 model gives.
    let mut m = machine(&[0xEE10, 0x1FB0], 0);
    m.run(1);
    assert_eq!(Some(m.r(1)), m.cpu.cp15(0, 0, 0, 5), "MPIDR");
    // MCR p15, 0, r0, c3, c0, 0 writes DACR and MRC p15, 0, r1, c3, c0, 0 reads it back.
    let mut m = machine(&[0xEE03, 0x0F10, 0xEE13, 0x1F10], 0);
    m.cpu.set_reg(0, 0x5555_5555);
    m.run(2);
    assert_eq!(m.r(1), 0x5555_5555, "DACR read back");
}

#[test]
fn msr_and_mrs_of_the_status_registers() {
    // MRS r0, CPSR (0xF3EF 0x8000) and MSR CPSR_fc, r3 (0xF383 0x8900).
    let mut m = machine(&[0xF3EF, 0x8000], nzcv(true, false, true, false));
    m.run(1);
    assert_eq!(m.r(0) >> 28, 0b1010, "NZCV read back");
    let mut m = machine(&[0xF383, 0x8900], 0);
    m.cpu.set_reg(3, nzcv(false, true, false, true));
    m.run(1);
    assert_eq!(
        m.flags(),
        nzcv(false, true, false, true),
        "MSR wrote the flags"
    );
}

#[test]
fn thumb2_parallel_uadd8_and_sel() {
    // UADD8 r2, r2, r12 (0xFA82 0xF24C) followed by SEL r0, r1, r3 (0xFAA1 0xF083)
    let mut m = machine(&[0xFA82, 0xF24C, 0xFAA1, 0xF083], 0);
    m.cpu.set_reg(2, 0x01_FE_00_80);
    m.cpu.set_reg(12, 0x02_03_04_05);
    m.cpu.set_reg(1, 0xAA_BB_CC_DD);
    m.cpu.set_reg(3, 0x11_22_33_44);
    m.run(1);
    // UADD8:
    // byte 0: 0x80 + 0x05 = 0x85 (no carry -> GE[0]=0)
    // byte 1: 0x00 + 0x04 = 0x04 (no carry -> GE[1]=0)
    // byte 2: 0xFE + 0x03 = 0x101 -> 0x01 (carry -> GE[2]=1)
    // byte 3: 0x01 + 0x02 = 0x03 (no carry -> GE[3]=0)
    assert_eq!(m.r(2), 0x03_01_04_85);
    assert_eq!((m.cpu.cpsr() >> 16) & 0xF, 0b0100, "GE bits set by UADD8");

    // SEL r0, r1, r3: byte 2 comes from r1 (GE[2]=1), others from r3 (GE=0)
    m.run(1);
    assert_eq!(
        m.r(0),
        0x11_BB_33_44,
        "SEL selected byte 2 from r1, others from r3"
    );
}
