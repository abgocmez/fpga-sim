//! Seeded mutation fuzzing of the project parser and validator (spec §14.10).
//!
//! Every `examples/*/project.toml` and a few handwritten projects are mutated
//! by a fixed-seed PRNG (xorshift, no dependency, same style as `crates/boot`).
//! The text goes through `Project::from_toml` (and, for a sample of the cases,
//! through `Project::load` from a file) and then `validate`. The properties are:
//! never panic, and the parser answers `Ok` or an `ApiError` with a message.
//! The same seeds give the same inputs on every run, so a failure reproduces.
//! This is not coverage-guided fuzzing; that lives in `fuzz/` (cargo-fuzz).

use sim_api::{ApiError, ErrorCode, Project};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// Mutated inputs per base file.
const CASES_PER_FILE: usize = 2000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Tokens that stress the TOML grammar and the number and string parsers.
const TOKENS: &[&[u8]] = &[
    b"\0",
    b"[",
    b"]]",
    b"[[x]]",
    b"\"",
    b"\"\"\"",
    b"'",
    b"=",
    b"{",
    b"}",
    b"\\",
    b"#",
    b"\n",
    b"\r\n",
    b"\t",
    b"\xff",
    b"\xc3\x28",
    "\u{202e}".as_bytes(),
    "é".as_bytes(),
    "😀".as_bytes(),
    "\u{feff}".as_bytes(),
];

/// Numbers and literals that overflow or confuse integer and float fields.
const HUGE: &[&str] = &[
    "99999999999999999999999999999999",
    "-9223372036854775809",
    "18446744073709551616",
    "4294967296",
    "-1",
    "1e400",
    "-1e400",
    "nan",
    "inf",
    "-inf",
    "0x",
    "0xFFFFFFFFFFFFFFFFFF",
    "1_000__000",
    "\"\"",
    "[]",
    "{}",
];

/// One mutation of `text`, chosen by the PRNG.
fn mutate_once(rng: &mut Rng, text: &mut Vec<u8>) {
    match rng.below(9) {
        // Byte flip.
        0 if !text.is_empty() => {
            let i = rng.below(text.len());
            text[i] ^= 1 << rng.below(8);
        }
        // Random byte.
        1 if !text.is_empty() => {
            let i = rng.below(text.len());
            text[i] = rng.next() as u8;
        }
        // Truncation.
        2 => {
            let n = rng.below(text.len() + 1);
            text.truncate(n);
        }
        // Inserted garbage token.
        3 => {
            let i = rng.below(text.len() + 1);
            let tok = TOKENS[rng.below(TOKENS.len())];
            text.splice(i..i, tok.iter().copied());
        }
        // Duplicated or deleted line.
        4 | 5 => {
            let mut lines: Vec<Vec<u8>> = text.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
            let i = rng.below(lines.len());
            if rng.below(2) == 0 {
                let dup = lines[i].clone();
                lines.insert(i, dup);
            } else if lines.len() > 1 {
                lines.remove(i);
            }
            *text = lines.join(&b'\n');
        }
        // Huge or odd value on a `key = value` line.
        6 => {
            let mut lines: Vec<Vec<u8>> = text.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
            let eqs: Vec<usize> = (0..lines.len())
                .filter(|&i| lines[i].contains(&b'='))
                .collect();
            if let Some(&i) = eqs.get(rng.below(eqs.len().max(1))) {
                let at = lines[i].iter().position(|b| *b == b'=').unwrap_or(0) + 1;
                let mut v = b" ".to_vec();
                v.extend_from_slice(HUGE[rng.below(HUGE.len())].as_bytes());
                lines[i].truncate(at);
                lines[i].extend_from_slice(&v);
            }
            *text = lines.join(&b'\n');
        }
        // Unicode or NUL inserted at a random point.
        7 | 8 => {
            let i = rng.below(text.len() + 1);
            let tok = TOKENS[rng.below(TOKENS.len())];
            text.splice(i..i, tok.iter().copied());
        }
        _ => {}
    }
}

fn mutate(rng: &mut Rng, base: &[u8]) -> Vec<u8> {
    let mut text = base.to_vec();
    for _ in 0..1 + rng.below(4) {
        mutate_once(rng, &mut text);
    }
    text
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Handwritten projects that the examples do not cover well.
fn handwritten() -> Vec<(&'static str, String)> {
    vec![
        (
            "minimal",
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n".to_string(),
        ),
        (
            "nested_tables",
            concat!(
                "schema_version = 1\n",
                "[board]\nname = \"zybo-z7-20\"\nddr_mb = 1024\n",
                "[backends]\nps = \"demo\"\npl = \"native\"\n",
                "[sim]\nquantum_ns = 1000\nseed = 7\n",
                "[native]\nmy_math_ip_0 = \"my_math_ip\"\n",
                "[native_params.my_math_ip_0]\nDEPTH = 16\nENABLE = true\nNAME = \"x\"\n",
                "[connect]\n\"my_math_ip_0.S_AXI\" = \"M_AXI_GP0@0x43c00000\"\n",
            )
            .to_string(),
        ),
        (
            "dotted_and_inline",
            concat!(
                "schema_version = 1\n",
                "board = { name = \"pynq-z2\" }\n",
                "sim.quantum_ns = 500\n",
                "devices = { led0 = \"led\", sw0 = \"switch\" }\n",
                "device_values.led0 = { brightness = 0.5 }\n",
            )
            .to_string(),
        ),
        (
            "boot_and_debug",
            concat!(
                "schema_version = 1\n",
                "[board]\nname = \"zybo-z7-10\"\n",
                "[boot]\nmode = \"boot_bin\"\nfile = \"BOOT.BIN\"\n",
                "[cpu0]\nelf = \"app.elf\"\n",
                "[debug]\ngdb = true\n",
            )
            .to_string(),
        ),
        ("empty", String::new()),
    ]
}

/// Parses and validates one candidate, returning a description of a failure.
fn check(text: &str, base: &Path) -> Result<bool, String> {
    match Project::from_toml(text) {
        Ok(p) => {
            let _ = p.validate(base);
            Ok(true)
        }
        Err(ApiError { code, message, .. }) => {
            if code == ErrorCode::InvalidArgument && !message.is_empty() {
                Ok(false)
            } else {
                Err(format!("unstructured parser error {code:?}: {message}"))
            }
        }
    }
}

struct Base {
    name: String,
    text: Vec<u8>,
    dir: PathBuf,
}

fn bases() -> Vec<Base> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("examples directory")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.join("project.toml").is_file())
        .collect();
    dirs.sort();
    for d in dirs {
        let text = std::fs::read(d.join("project.toml")).expect("read example");
        out.push(Base {
            name: d.file_name().unwrap().to_string_lossy().into_owned(),
            text,
            dir: d,
        });
    }
    let scratch = std::env::temp_dir().join(format!("zynqsim-fuzz-project-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    for (name, text) in handwritten() {
        out.push(Base {
            name: format!("handwritten/{name}"),
            text: text.into_bytes(),
            dir: scratch.clone(),
        });
    }
    out
}

#[test]
fn mutated_projects_never_panic_and_are_answered_structurally() {
    let bases = bases();
    assert!(
        bases.len() >= 27 + 5,
        "expected the examples and handwritten projects, got {}",
        bases.len()
    );
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut failure: Option<String> = None;

    'outer: for (bi, base) in bases.iter().enumerate() {
        // The unmutated project must parse: the baseline the mutations start from.
        let original = String::from_utf8_lossy(&base.text).into_owned();
        assert!(
            check(&original, &base.dir).is_ok(),
            "{} does not parse unmutated",
            base.name
        );
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (bi as u64 + 1));
        for case in 0..CASES_PER_FILE {
            let mutated = mutate(&mut rng, &base.text);
            let text = String::from_utf8_lossy(&mutated).into_owned();
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| check(&text, &base.dir)));
            match outcome {
                Ok(Ok(true)) => accepted += 1,
                Ok(Ok(false)) => rejected += 1,
                Ok(Err(why)) => {
                    failure = Some(format!(
                        "{} case {case}: {why}\ninput (hex): {}",
                        base.name,
                        hex(&mutated)
                    ));
                    break 'outer;
                }
                Err(_) => {
                    failure = Some(format!(
                        "{} case {case}: PANIC\ninput (hex): {}",
                        base.name,
                        hex(&mutated)
                    ));
                    break 'outer;
                }
            }
            // A sample goes through the file path too, so `Project::load` (UTF-8
            // check and read) is covered as well as `from_toml`.
            if case % 100 == 0 {
                let path = std::env::temp_dir().join(format!(
                    "zynqsim-fuzz-{}-{bi}-{case}.toml",
                    std::process::id()
                ));
                std::fs::write(&path, &mutated).expect("write fuzz input");
                let loaded = panic::catch_unwind(AssertUnwindSafe(|| Project::load(&path)));
                let _ = std::fs::remove_file(&path);
                match loaded {
                    Ok(Ok(_)) | Ok(Err(_)) => {}
                    Err(_) => {
                        failure = Some(format!(
                            "{} case {case}: PANIC in Project::load\ninput (hex): {}",
                            base.name,
                            hex(&mutated)
                        ));
                        break 'outer;
                    }
                }
            }
        }
    }

    if let Some(f) = failure {
        panic!("{f}");
    }
    // Both outcomes must occur or the mutations exercise nothing interesting.
    assert!(
        accepted > 1000 && rejected > 1000,
        "accepted={accepted} rejected={rejected}"
    );
}
