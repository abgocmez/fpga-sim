//! Seeded mutation fuzzing of the front end: 5000 corrupted copies of repository
//! sources must never panic, and every run stops within its step budget.

use rtl_frontend::{Options, parse_text};
use std::path::{Path, PathBuf};

/// Deterministic xorshift generator (no external crates).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fragments that exercise the lexer, the preprocessor and the parser.
const DICT: &[&str] = &[
    "begin",
    "end",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ";",
    ",",
    ":",
    "#",
    "@",
    "`define",
    "`ifdef",
    "`endif",
    "`else",
    "`include",
    "`X",
    "\"",
    "/*",
    "*/",
    "//",
    "\\",
    "'",
    "'h",
    "8'b",
    "module",
    "endmodule",
    "always",
    "assign",
    "case",
    "endcase",
    "generate",
    "endgenerate",
    "for",
    "if",
    "=",
    "<=",
    "+:",
    "::",
    "$display",
    "$clog2(",
    "function",
    "endfunction",
    "typedef",
    "enum",
    "struct",
    "class",
    "interface",
    "import",
    "parameter",
    "localparam",
    "\n",
    " ",
    "\\\n",
    "*)",
    "(*",
    "1.5",
    "_",
    "x",
    "z",
];

fn sources() -> Vec<Vec<u8>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let picks = [
        "fixtures/rtl/math/my_math_ip.v",
        "fixtures/rtl/pmod/cosim_top.sv",
        "tests/rtl/picorv32/rtl/picorv32.v",
        "tests/rtl/forencich-uart/rtl/uart_tx.v",
        "tests/rtl/pulp-common-cells/rtl/cc_fifo.sv",
        "tests/rtl/dpretet-async-fifo/rtl/wptr_full.v",
        "tests/rtl/probe-covergroup/top.sv",
        "fixtures/export/postsynth/cosim_top_func_synth.v",
    ];
    let mut out = Vec::new();
    for p in picks {
        let path: PathBuf = root.join(p);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // A window of each file keeps the cases fast without losing structure.
        out.push(bytes.into_iter().take(6000).collect());
    }
    out
}

fn mutate(rng: &mut Rng, base: &[u8], others: &[Vec<u8>]) -> Vec<u8> {
    let mut v = base.to_vec();
    let steps = 1 + rng.below(8);
    for _ in 0..steps {
        if v.is_empty() {
            v.extend_from_slice(DICT[rng.below(DICT.len())].as_bytes());
            continue;
        }
        match rng.below(7) {
            0 => {
                // Delete a span.
                let a = rng.below(v.len());
                let n = 1 + rng.below(40);
                let b = (a + n).min(v.len());
                v.drain(a..b);
            }
            1 => {
                // Insert a dictionary fragment.
                let a = rng.below(v.len() + 1);
                let frag = DICT[rng.below(DICT.len())].as_bytes().to_vec();
                v.splice(a..a, frag);
            }
            2 => {
                // Overwrite a byte with printable noise.
                let a = rng.below(v.len());
                v[a] = (32 + rng.below(95)) as u8;
            }
            3 => {
                // Truncate.
                let a = rng.below(v.len());
                v.truncate(a);
            }
            4 => {
                // Copy a span from another source (splices unrelated constructs together).
                let other = &others[rng.below(others.len())];
                if !other.is_empty() {
                    let s = rng.below(other.len());
                    let n = 1 + rng.below(200);
                    let e = (s + n).min(other.len());
                    let at = rng.below(v.len() + 1);
                    let chunk = other[s..e].to_vec();
                    v.splice(at..at, chunk);
                }
            }
            5 => {
                // Duplicate a line.
                let a = rng.below(v.len());
                let start = v[..a]
                    .iter()
                    .rposition(|&c| c == b'\n')
                    .map_or(0, |p| p + 1);
                let end = v[a..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(v.len(), |p| a + p + 1);
                let line = v[start..end].to_vec();
                v.splice(start..start, line);
            }
            _ => {
                // Flip a bit (keeps the value a byte; may create non-UTF-8, which is allowed).
                let a = rng.below(v.len());
                v[a] ^= 1 << rng.below(8);
            }
        }
    }
    v
}

#[test]
fn five_thousand_mutated_sources_never_panic_and_stop_within_the_budget() {
    let srcs = sources();
    let opts = Options {
        step_limit: 300_000,
        ..Options::default()
    };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut budget_hits = 0usize;
    let mut with_diags = 0usize;
    for case in 0..5000 {
        let base = &srcs[case % srcs.len()];
        let text = mutate(&mut rng, base, &srcs);
        let text = String::from_utf8_lossy(&text).into_owned();
        let parsed = parse_text(&format!("fuzz{case}.sv"), &text, &opts);
        if parsed.diagnostics.iter().any(|d| d.construct == "budget") {
            budget_hits += 1;
        }
        if !parsed.diagnostics.is_empty() {
            with_diags += 1;
        }
        for d in &parsed.diagnostics {
            assert!(
                !d.construct.is_empty(),
                "case {case}: a diagnostic without a construct: {d:?}"
            );
            assert!(
                d.line >= 1 || d.file.is_empty() || d.construct == "file",
                "case {case}: {d:?}"
            );
        }
    }
    println!(
        "fuzz: 5000 cases, {with_diags} with diagnostics, {budget_hits} stopped by the budget"
    );
    assert_eq!(
        budget_hits, 0,
        "the budget should be far above what a 6 KB mutated source needs"
    );
}

/// Regression: an apostrophe followed by a letter that is not a base used to rewind
/// the lexer to the apostrophe and loop forever (found by the fuzzer).
#[test]
fn apostrophe_before_a_non_base_letter_terminates() {
    for src in [
        "module m; wire a = 'x_y; endmodule\n",
        "'q",
        "4's",
        "8'sq 1",
    ] {
        let p = parse_text("t.sv", src, &Options::default());
        assert!(
            !p.diagnostics.is_empty(),
            "{src:?} is invalid and must be reported"
        );
    }
}

/// The step budget bounds a large input: a tiny budget stops the parse with a
/// `budget` diagnostic instead of producing a full tree.
#[test]
fn the_step_budget_stops_a_large_input() {
    let mut src = String::new();
    for i in 0..2000 {
        src.push_str(&format!("module m{i}; wire a{i} = b{i} & c; endmodule\n"));
    }
    let opts = Options {
        step_limit: 500,
        ..Options::default()
    };
    let p = parse_text("big.sv", &src, &opts);
    assert!(
        p.diagnostics.iter().any(|d| d.construct == "budget"),
        "budget must be reported"
    );
    assert!(p.units[0].items.len() < 2000, "the parse must stop early");
}
