//! The prebuilt Rust firmware ELFs (`fixtures/elf/rust_*.elf`) loaded into RAM at their link
//! addresses and run on the interpreter, with UART1 captured at 0xE0001030.

mod common;

use common::{Outcome, fixture, parse_elf, read_fixture, run_firmware};

/// Step budget per image: far more than any of them needs to print and park.
const BUDGET: u64 = 5_000_000;

#[test]
fn rust_hello_runs_to_completion_and_prints_its_message() {
    let elf = parse_elf(&read_fixture("rust_hello.elf"));
    let run = run_firmware(&elf, BUDGET);
    assert!(
        matches!(run.outcome, Outcome::Looped { .. }),
        "rust_hello should park in its final loop, got {:?} after {} steps",
        run.outcome,
        run.steps
    );
    assert!(
        run.uart
            .contains("hello from Rust on the simulated Cortex-A9"),
        "uart: {:?}",
        run.uart
    );
    assert!(
        run.uart.contains("sum(1..=100) = 5050"),
        "uart: {:?}",
        run.uart
    );
    assert!(
        run.uart.contains("RESULT: ALL PASS"),
        "uart: {:?}",
        run.uart
    );
}

/// Every distinct instruction word of the 35 firmware images (listed in `tests/data`, from
/// `objdump -d`) must decode to an implemented instruction, when executed with no condition
/// failure in a fresh core. A word that reads as data is not in the list, so none is.
#[test]
fn every_instruction_word_of_the_firmware_decodes() {
    use cpu_a9::{Cpu, StepResult};
    use ps_bus::RamBus;

    let list = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("rust_firmware_insn_words.txt"),
    )
    .expect("word list present");
    let mut missing = Vec::new();
    let mut count = 0usize;
    for line in list
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let word = u32::from_str_radix(line.trim(), 16).expect("hex word");
        count += 1;
        // A word whose condition fails is a NOP and is not decoded, so run it under NZCV
        // states that together pass all fifteen conditions: 0000, 1111, 0010 and 1000.
        for nzcv in [0x0u32, 0xF, 0x2, 0x8] {
            let mut bus = RamBus::new(0x1000, 0x100);
            bus.load(0x1000, &word.to_le_bytes());
            let mut cpu = Cpu::new(0);
            cpu.reset(0x1000);
            cpu.set_cpsr(0x10 | (nzcv << 28));
            if let StepResult::Unimplemented { insn, .. } = cpu.step(&mut bus) {
                missing.push(format!("{insn:08x}"));
                break;
            }
        }
    }
    assert!(
        count > 10_000,
        "the word list looks truncated ({count} words)"
    );
    assert!(
        missing.is_empty(),
        "{} of {count} firmware instruction words are not implemented, e.g. {:?}",
        missing.len(),
        &missing[..missing.len().min(16)]
    );
}

/// Prints, for every Rust firmware image, how the interpreter fares. This is the table in
/// `docs/adr/030-cpu-a9.md`; run with `--nocapture`. It asserts nothing about the results.
#[test]
fn every_rust_firmware_image_is_classified() {
    let mut names: Vec<String> = std::fs::read_dir(fixture(""))
        .expect("fixtures/elf exists")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("rust_") && n.ends_with(".elf"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no rust_*.elf fixtures found");
    for name in names {
        let elf = parse_elf(&read_fixture(&name));
        let run = run_firmware(&elf, BUDGET);
        let first_line = run
            .uart
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();
        let result = run.uart.contains("RESULT: ALL PASS");
        let outcome = match &run.outcome {
            Outcome::Looped { pc } => format!("parks at {pc:#010x}"),
            Outcome::Idle { pc } => format!("WFI/WFE idle at {pc:#010x} (no interrupt source)"),
            Outcome::Finished => "end-of-test write".to_string(),
            Outcome::Unimplemented { pc, insn } => {
                format!("UNIMPLEMENTED {insn:#010x} at {pc:#010x}")
            }
            Outcome::Budget => "budget exhausted".to_string(),
        };
        let touched = run.touched.join(",");
        let events = run.events.join(",");
        let aborts = run.aborts.join(",");
        let unmapped: Vec<String> = run.unmapped.iter().map(|a| format!("{a:#010x}")).collect();
        eprintln!(
            "RUSTFW {name}: steps={} outcome=[{outcome}] all_pass={result} uart_bytes={} first=\"{first_line}\" touched=[{touched}] events=[{events}] aborts=[{aborts}] unmapped=[{}] sctlr={:#010x}",
            run.steps,
            run.uart.len(),
            unmapped.join(","),
            run.sctlr
        );
    }
}
