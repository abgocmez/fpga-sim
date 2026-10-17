//! The third-party corpus (`tests/rtl/**`) and the repository's own RTL fixtures run on
//! the native engine. Each corpus design has a self-checking testbench that prints
//! `CORPUS_PASS` or `CORPUS_FAIL`; the table of results is written to
//! `docs/CORPUS_NATIVE.md` and pinned below, so a change in either direction fails.

use rtl_engine::{Design, Simulator};
use std::path::{Path, PathBuf};

/// Simulated time each corpus testbench may use (10 ms).
const LIMIT_PS: u64 = 10_000_000_000;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Verilog and SystemVerilog files of a directory, sorted.
fn sources_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("v" | "sv")))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Design name.
    pub name: String,
    /// Elaborated with no diagnostic and the engine accepted it.
    pub accepted: bool,
    /// Ran to `$finish` or to the time limit without a run-time error.
    pub runs: bool,
    /// Printed `CORPUS_PASS`.
    pub passes: bool,
    /// What happened, in a short phrase.
    pub note: String,
}

fn run_bench(name: &str, sources: &[PathBuf], include: &[PathBuf], top: &str) -> Row {
    let design = Design::elaborate(top, sources, &[], include);
    if !design.diagnostics.is_empty() {
        let d = &design.diagnostics[0];
        return Row {
            name: name.into(),
            accepted: false,
            runs: false,
            passes: false,
            note: format!(
                "refused: {} at {}:{} ({} in all): {}",
                d.construct,
                Path::new(&d.file)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("?"),
                d.line,
                design.diagnostics.len(),
                d.message
            ),
        };
    }
    let mut sim: Simulator = match Simulator::new(design) {
        Ok(s) => s,
        Err(d) => {
            return Row {
                name: name.into(),
                accepted: false,
                runs: false,
                passes: false,
                note: format!("start: {}", d[0]),
            };
        }
    };
    match sim.run_until(LIMIT_PS) {
        Err(e) => Row {
            name: name.into(),
            accepted: true,
            runs: false,
            passes: false,
            note: format!("run error: {}", e.message),
        },
        Ok(()) => {
            let log = sim.log().join("\n");
            let passes = log.contains("CORPUS_PASS");
            let fails = log.contains("CORPUS_FAIL");
            let note = if passes {
                "CORPUS_PASS".to_string()
            } else if fails {
                "CORPUS_FAIL".to_string()
            } else if sim.finished() {
                "finished without a verdict".to_string()
            } else {
                "time limit reached without a verdict".to_string()
            };
            Row {
                name: name.into(),
                accepted: true,
                runs: true,
                passes,
                note,
            }
        }
    }
}

/// The corpus designs: name and the directory under `tests/rtl`.
const CORPUS: [(&str, &str); 9] = [
    ("forencich-lfsr", "forencich-lfsr"),
    ("forencich-uart", "forencich-uart"),
    ("forencich-axis", "forencich-axis"),
    ("dpretet-async-fifo", "dpretet-async-fifo"),
    ("picorv32", "picorv32"),
    ("serv", "serv"),
    ("uriscv", "uriscv"),
    ("pulp-common-cells", "pulp-common-cells"),
    ("vivado-funcsim", "vivado-funcsim"),
];

fn corpus_rows() -> Vec<Row> {
    let root = workspace().join("tests").join("rtl");
    CORPUS
        .iter()
        .map(|(name, dir)| {
            let base = root.join(dir);
            let mut srcs = sources_in(&base.join("rtl"));
            let tb = sources_in(&base.join("tb"));
            if tb.is_empty() {
                return Row {
                    name: name.to_string(),
                    accepted: false,
                    runs: false,
                    passes: false,
                    note: "no self-checking testbench".into(),
                };
            }
            srcs.extend(tb);
            let mut include = vec![base.join("rtl"), base.join("tb"), base.join("include")];
            include.retain(|p| p.is_dir());
            run_bench(name, &srcs, &include, "corpus_tb")
        })
        .collect()
}

/// Fixtures of the repository without a testbench of their own: elaboration only.
fn fixture_rows() -> Vec<Row> {
    let fx = workspace().join("fixtures").join("rtl");
    let cases: [(&str, &str, &str); 3] = [
        ("irq_ip (fixtures/rtl/irq)", "irq/irq_ip.v", "irq_ip"),
        (
            "oled_spi (fixtures/rtl/pmod)",
            "pmod/oled_spi.v",
            "oled_spi",
        ),
        ("rgb_pwm (fixtures/rtl/pmod)", "pmod/rgb_pwm.v", "rgb_pwm"),
    ];
    cases
        .iter()
        .map(|(name, file, top)| {
            let src = vec![fx.join(file)];
            let design = Design::elaborate(top, &src, &[], &[]);
            let note = if design.diagnostics.is_empty() {
                match Simulator::new(design) {
                    Ok(_) => "elaborates; no testbench (not run)".to_string(),
                    Err(d) => format!("start: {}", d[0]),
                }
            } else {
                let d = &design.diagnostics[0];
                format!("refused: {} at line {}", d.construct, d.line)
            };
            Row {
                name: name.to_string(),
                accepted: note.starts_with("elaborates"),
                runs: false,
                passes: false,
                note,
            }
        })
        .collect()
}

fn markdown(rows: &[Row], fixtures: &[Row]) -> String {
    let mut s = String::from(
        "# Corpus on the native RTL engine (generated)\n\n\
         Written by `cargo test -p rtl-engine --test corpus`. Each row is one design from\n\
         `tests/rtl` run with its `tb/corpus_tb.sv` (top `corpus_tb`) for at most 10 ms of\n\
         simulated time. *accepted* means the front end and the engine's elaboration took it\n\
         with no diagnostic. *runs* means no run-time error. *passes* means the testbench printed\n\
         `CORPUS_PASS`. The engine is 2-state (x and z read as 0); a testbench that depends on\n\
         x or z comparisons is not decided the same way as in XSim.\n\n\
         | design | accepted | runs | passes | note |\n|---|---|---|---|---|\n",
    );
    for r in rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            r.name,
            yn(r.accepted),
            yn(r.runs),
            yn(r.passes),
            r.note
        ));
    }
    let total = rows.len();
    let ok = rows.iter().filter(|r| r.passes).count();
    s.push_str(&format!("\n**Corpus: {ok} of {total} pass.**\n\n"));
    s.push_str("## Fixtures without a testbench\n\n| design | accepted | note |\n|---|---|---|\n");
    for r in fixtures {
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            r.name,
            yn(r.accepted),
            r.note
        ));
    }
    s
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The pinned outcome: (design, accepted, runs, passes). Each refusal is a real limit
/// of the engine (see the note in the generated table and `docs/RTL_SUBSET.md`).
const PINNED: [(&str, bool, bool, bool); 9] = [
    ("forencich-lfsr", true, true, true),
    ("forencich-uart", true, true, true),
    ("forencich-axis", true, true, true),
    ("dpretet-async-fifo", true, true, true),
    ("picorv32", true, true, true),
    ("serv", true, true, true),
    ("uriscv", false, false, false),
    ("pulp-common-cells", false, false, false),
    ("vivado-funcsim", false, false, false),
];

#[test]
fn corpus_results_are_pinned_and_written_to_docs() {
    let rows = corpus_rows();
    let fixtures = fixture_rows();
    let doc = workspace().join("docs").join("CORPUS_NATIVE.md");
    if doc.parent().is_some_and(|p| p.is_dir()) {
        let _ = std::fs::write(&doc, markdown(&rows, &fixtures));
    }
    for r in &rows {
        eprintln!(
            "{:24} accepted={:5} runs={:5} passes={:5} {}",
            r.name, r.accepted, r.runs, r.passes, r.note
        );
    }
    for (name, accepted, runs, passes) in PINNED {
        let r = rows.iter().find(|r| r.name == name).expect("row exists");
        assert_eq!(
            (r.accepted, r.runs, r.passes),
            (accepted, runs, passes),
            "{name}: {}",
            r.note
        );
    }
}
