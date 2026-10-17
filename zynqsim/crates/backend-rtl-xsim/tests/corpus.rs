//! The universality corpus (prompt §14.5): third-party Verilog / SystemVerilog designs with their
//! licences, a Vivado post-synthesis functional netlist and constructs written for this corpus,
//! each compiled, elaborated and run in XSim *without* a PS (self-checking testbenches), plus a
//! negative test for every construct the flow does not take.
//!
//! * `corpus_<design>` : the design compiles, runs and passes its own check.
//! * `probe_<name>`    : a language construct XSim accepts (bind, virtual modports, ...).
//! * `negative_<name>` : an unsupported construct; the diagnostic names file, line and construct.
//! * `corpus_table_is_current` : regenerates the tables of `docs/CORPUS.md` from the runs above
//!   and compares them with the file; `ZYNQSIM_CORPUS_BLESS=1` (or `cargo xtask corpus`) rewrites
//!   it, including the (not compared) wall times.
//!
//! Needs Vivado (XSim). When absent the tests print `SKIPPED:` and return.

use backend_rtl_xsim::{
    DiagSeverity, StandaloneConfig, StandaloneReport, StandaloneSource, run_standalone,
};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use xsim_export::Lang;

// ------------------------------------------------------------------------------------------
// Manifest
// ------------------------------------------------------------------------------------------

/// How a design reports success.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The testbench prints `CORPUS_PASS <name>` and never `CORPUS_FAIL`.
    Marker,
    /// Upstream's own testbench: `test cases completed successfully`, never `did not complete`.
    Upstream,
}

/// What the corpus expects of a design on this backend.
#[derive(Clone, Copy)]
enum Expect {
    Pass,
    /// It does not pass; the string must appear in the summary (the reason, from a diagnostic).
    #[allow(dead_code)] // no design is expected to fail today; kept for the table's contract
    Unsupported(&'static str),
}

struct Design {
    name: &'static str,
    upstream: &'static str,
    commit: &'static str,
    licence: &'static str,
    /// Design files, relative to `tests/rtl/<name>`, in compile order.
    rtl: &'static [&'static str],
    /// Testbench files.
    tb: &'static [&'static str],
    top: &'static str,
    includes: &'static [&'static str],
    defines: &'static [&'static str],
    libs: &'static [&'static str],
    extra_tops: &'static [&'static str],
    /// Compile `.v` files as SystemVerilog (`xvlog -sv`).
    sv: bool,
    /// Extra `xvlog` arguments.
    xvlog: &'static [&'static str],
    /// Extra `xelab` arguments.
    xelab: &'static [&'static str],
    pass: Pass,
    expect: Expect,
    notes: &'static str,
}

const D: Design = Design {
    name: "",
    upstream: "",
    commit: "",
    licence: "",
    rtl: &[],
    tb: &[],
    top: "corpus_tb",
    includes: &[],
    defines: &[],
    libs: &[],
    extra_tops: &[],
    sv: false,
    xvlog: &[],
    xelab: &[],
    pass: Pass::Marker,
    expect: Expect::Pass,
    notes: "",
};

static DESIGNS: &[Design] = &[
    Design {
        name: "secworks-sha256",
        upstream: "https://github.com/secworks/sha256",
        commit: "837c5cc396f001d18f2c765721c585716eb439ae",
        licence: "BSD-2-Clause",
        rtl: &[
            "rtl/sha256_k_constants.v",
            "rtl/sha256_w_mem.v",
            "rtl/sha256_core.v",
            "rtl/sha256.v",
        ],
        tb: &["tb/tb_sha256.v"],
        top: "tb_sha256",
        xvlog: &["--relax"],
        pass: Pass::Upstream,
        notes: "SHA-224/256 core, upstream testbench with NIST vectors",
        ..D
    },
    Design {
        name: "secworks-sha1",
        upstream: "https://github.com/secworks/sha1",
        commit: "50f82f4ca2f8194fd47d8360270ac7dd141d53d1",
        licence: "BSD-2-Clause",
        rtl: &["rtl/sha1_w_mem.v", "rtl/sha1_core.v", "rtl/sha1.v"],
        tb: &["tb/tb_sha1.v"],
        top: "tb_sha1",
        pass: Pass::Upstream,
        notes: "SHA-1 core, upstream testbench",
        ..D
    },
    Design {
        name: "secworks-md5",
        upstream: "https://github.com/secworks/md5",
        commit: "54a6308d4d432daf4f6c6baa8c56ec204f106b4e",
        licence: "BSD-2-Clause",
        rtl: &["rtl/md5_core.v", "rtl/md5.v"],
        tb: &["tb/tb_md5.v"],
        top: "tb_md5",
        pass: Pass::Upstream,
        notes: "MD5 core, upstream testbench",
        ..D
    },
    Design {
        name: "secworks-aes",
        upstream: "https://github.com/secworks/aes",
        commit: "80dc4718e1dcbbdb4b0dd1bdb393d8f7b98981dc",
        licence: "BSD-2-Clause",
        rtl: &[
            "rtl/aes_sbox.v",
            "rtl/aes_inv_sbox.v",
            "rtl/aes_key_mem.v",
            "rtl/aes_encipher_block.v",
            "rtl/aes_decipher_block.v",
            "rtl/aes_core.v",
            "rtl/aes.v",
        ],
        tb: &["tb/tb_aes.v"],
        top: "tb_aes",
        pass: Pass::Upstream,
        notes: "AES-128/256 encrypt and decrypt, upstream testbench with NIST vectors",
        ..D
    },
    Design {
        name: "secworks-chacha",
        upstream: "https://github.com/secworks/chacha",
        commit: "7eaba360df9fed9fc2db98d5f3df81cf01e5b604",
        licence: "BSD-2-Clause",
        rtl: &["rtl/chacha_qr.v", "rtl/chacha_core.v", "rtl/chacha.v"],
        tb: &["tb/tb_chacha.v"],
        top: "tb_chacha",
        pass: Pass::Upstream,
        notes: "ChaCha stream cipher, upstream testbench with RFC vectors",
        ..D
    },
    Design {
        name: "forencich-lfsr",
        upstream: "https://github.com/alexforencich/verilog-lfsr",
        commit: "c1f86d036745935f3812ec6ddc7821c70ef4c9de",
        licence: "MIT",
        rtl: &["rtl/lfsr.v", "rtl/lfsr_crc.v"],
        tb: &["tb/corpus_tb.sv"],
        notes: "parallel CRC generator; CRC-32 and CRC-16/XMODEM check values of \"123456789\"",
        ..D
    },
    Design {
        name: "forencich-uart",
        upstream: "https://github.com/alexforencich/verilog-uart",
        commit: "1b867e53af738e4a8bc7c839ca2f1c07f40382dc",
        licence: "MIT",
        rtl: &["rtl/uart_tx.v", "rtl/uart_rx.v"],
        tb: &["tb/corpus_tb.sv"],
        notes: "AXI-Stream UART, transmitter looped into receiver, 8 bytes",
        ..D
    },
    Design {
        name: "forencich-axis",
        upstream: "https://github.com/alexforencich/verilog-axis",
        commit: "48ff7a7e2ef782cf778d47910cf85835c64b1bce",
        licence: "MIT",
        rtl: &["rtl/axis_fifo.v", "rtl/axis_register.v"],
        tb: &["tb/corpus_tb.sv"],
        notes: "AXI-Stream FIFO + skid register with random stalls, 200 words in frames of 16",
        ..D
    },
    Design {
        name: "picorv32",
        upstream: "https://github.com/YosysHQ/picorv32",
        commit: "ef203c2b0a3fb793280f5114941416c425c5b461",
        licence: "ISC",
        rtl: &["rtl/picorv32.v"],
        tb: &["tb/corpus_tb.sv"],
        notes: "RV32IM CPU running a hand-assembled sum/multiply program",
        ..D
    },
    Design {
        name: "uriscv",
        upstream: "https://github.com/ultraembedded/core_uriscv",
        commit: "a6dc9bee3a99f693ba9f2ec526f41c54b4e191a2",
        licence: "Apache-2.0",
        rtl: &[
            "rtl/uriscv_defs.v",
            "rtl/uriscv_alu.v",
            "rtl/uriscv_branch.v",
            "rtl/uriscv_csr.v",
            "rtl/uriscv_lsu.v",
            "rtl/uriscv_muldiv.v",
            "rtl/riscv_core.v",
        ],
        tb: &["tb/tcm_mem_ram.v", "tb/tcm_mem.v", "tb/corpus_tb.sv"],
        xvlog: &["--relax"],
        notes: "RV32IM CPU with upstream's TCM testbench memory, same program as picorv32",
        ..D
    },
    Design {
        name: "pulp-common-cells",
        upstream: "https://github.com/pulp-platform/common_cells",
        commit: "e73baaec2ca665cd80c3c384e9258e35242b829c",
        licence: "SHL-0.51 (Apache-2.0 at the licensee's option)",
        rtl: &[
            "rtl/cc_pkg.sv",
            "rtl/cc_lzc.sv",
            "rtl/cc_popcount.sv",
            "rtl/cc_binary_to_gray.sv",
            "rtl/cc_gray_to_binary.sv",
            "rtl/cc_fifo.sv",
            "rtl/cc_rr_arb_tree.sv",
        ],
        tb: &["tb/corpus_tb.sv"],
        includes: &["include"],
        notes: "SystemVerilog: package, `parameter type`, `always_comb`, macro headers with                 assertions; popcount, lzc, gray, FIFO vs queue model, round-robin arbiter",
        ..D
    },
    Design {
        name: "serv",
        upstream: "https://github.com/olofk/serv",
        commit: "f200eb2ed7b69ac1c6b8eddd47654522aeee5ce8",
        licence: "ISC",
        rtl: &[
            "rtl/serv_aligner.v",
            "rtl/serv_alu.v",
            "rtl/serv_bufreg.v",
            "rtl/serv_bufreg2.v",
            "rtl/serv_compdec.v",
            "rtl/serv_csr.v",
            "rtl/serv_ctrl.v",
            "rtl/serv_decode.v",
            "rtl/serv_immdec.v",
            "rtl/serv_mem_if.v",
            "rtl/serv_rf_if.v",
            "rtl/serv_rf_ram.v",
            "rtl/serv_rf_ram_if.v",
            "rtl/serv_rf_top.v",
            "rtl/serv_state.v",
            "rtl/serv_top.v",
        ],
        tb: &["tb/corpus_tb.sv"],
        notes: "bit-serial RV32I CPU with SRAM register file, Wishbone memory, hand-assembled program",
        ..D
    },
    Design {
        name: "dpretet-async-fifo",
        upstream: "https://github.com/dpretet/async_fifo",
        commit: "38c22208d3948833f275b917c920e02b1cdadf56",
        licence: "MIT",
        rtl: &[
            "rtl/sync_ptr.v",
            "rtl/sync_r2w.v",
            "rtl/sync_w2r.v",
            "rtl/wptr_full.v",
            "rtl/rptr_empty.v",
            "rtl/fifomem.v",
            "rtl/async_fifo.v",
        ],
        tb: &["tb/corpus_tb.sv"],
        notes: "dual-clock FIFO (100 MHz write, 71 MHz read), 500 words with random gaps",
        ..D
    },
];

/// The Vivado post-synthesis functional netlist.
static FUNCSIM: Design = Design {
    name: "vivado-funcsim",
    upstream: "Vivado 2022.1 `write_verilog -mode funcsim` of `my_math_ip` (fixtures/export)",
    commit: "-",
    licence: "Apache-2.0 (this repository)",
    rtl: &["../../../fixtures/export/my_math_ip_funcsim.v"],
    tb: &["tb/corpus_tb.sv"],
    libs: &["unisims_ver"],
    extra_tops: &["work.glbl"],
    notes: "UNISIM LUT/FDRE netlist, `-L unisims_ver`, its own `glbl`; AXI4-Lite read/write",
    ..D
};

const PROBES: &[(&str, &str)] = &[
    ("probe-bind", "`bind` of a checker module into a design"),
    (
        "probe-virtual-modport",
        "SystemVerilog interface, modports, class with a `virtual` interface handle",
    ),
    ("probe-config-block", "Verilog-2001 `config` block"),
    ("probe-covergroup", "functional coverage `covergroup`"),
    (
        "probe-constrained-random",
        "class `rand` fields and `constraint` solved by `randomize()`",
    ),
    (
        "probe-concurrent-assertion",
        "`property` and `assert property` with a cycle delay range",
    ),
];

/// UVM needs `-L uvm` (probe) and fails without it (negative `uvm_without_library`).
const UVM_PROBE: (&str, &str) = (
    "probe-uvm-with-library",
    "UVM 1.2 test with `xvlog -L uvm` / `xelab -L uvm`",
);

/// A fix that turns a negative into a pass: the configuration, not the tool, was missing.
#[derive(Default)]
struct Fix {
    defines: &'static [&'static str],
    libs: &'static [&'static str],
    /// Include directories relative to the case directory.
    includes: &'static [&'static str],
    lang: Option<Lang>,
    /// Extra `xvlog` arguments.
    xvlog: &'static [&'static str],
    /// Source files relative to the Vivado install, appended (Vivado's `glbl.v`).
    vivado_sources: &'static [&'static str],
    extra_tops: &'static [&'static str],
    says: &'static str,
}

/// How a negative case is expected to fail.
struct Neg {
    name: &'static str,
    what: &'static str,
    file: &'static str,
    top: &'static str,
    /// `failed_stage()` of the report: `xvlog`, `xvhdl`, `xelab`, or `-` when the design ran.
    stage: &'static str,
    severity: DiagSeverity,
    code: Option<&'static str>,
    line: u32,
    /// Vivado versions differ in which line a run-time warning names (the statement in 2022.1,
    /// the enclosing process in 2023.1); the diagnostic may be this many lines later.
    line_slack: u32,
    /// Contained in the diagnostic's construct.
    construct: &'static str,
    fix: Option<Fix>,
}

impl Neg {
    fn line_ok(&self, line: Option<u32>) -> bool {
        line.is_some_and(|l| l >= self.line && l <= self.line + self.line_slack)
    }
}

fn negatives() -> Vec<Neg> {
    let n = |name, what, file, stage, severity, code, line, construct| Neg {
        name,
        what,
        file,
        top: "top",
        stage,
        severity,
        code,
        line,
        line_slack: 0,
        construct,
        fix: None,
    };
    let e = DiagSeverity::Error;
    vec![
        Neg {
            fix: Some(Fix {
                includes: &["inc"],
                says: "`+incdir+inc`",
                ..Fix::default()
            }),
            ..n(
                "missing_include",
                "`include` of a header that is not on the include path",
                "top.sv",
                "xvlog",
                e,
                Some("VRFC 10-3195"),
                2,
                "no_such_header.vh",
            )
        },
        Neg {
            fix: Some(Fix {
                defines: &["DATA_WIDTH=8"],
                says: "`-d DATA_WIDTH=8`",
                ..Fix::default()
            }),
            ..n(
                "missing_define",
                "macro used but its `-d` define is missing",
                "top.sv",
                "xvlog",
                e,
                Some("VRFC 10-3805"),
                3,
                "DATA_WIDTH",
            )
        },
        Neg {
            fix: Some(Fix {
                libs: &["unisims_ver"],
                vivado_sources: &["data/verilog/src/glbl.v"],
                extra_tops: &["work.glbl"],
                says: "`-L unisims_ver` and Vivado's `glbl.v`",
                ..Fix::default()
            }),
            ..n(
                "missing_unisim_library",
                "UNISIM primitive (`FDRE`) without the `unisims_ver` library",
                "top.sv",
                "xelab",
                e,
                Some("VRFC 10-2063"),
                5,
                "FDRE",
            )
        },
        n(
            "missing_module",
            "instance of a module no source provides (vendor black box)",
            "top.sv",
            "xelab",
            e,
            Some("VRFC 10-2063"),
            4,
            "vendor_black_box",
        ),
        Neg {
            fix: Some(Fix {
                libs: &["uvm"],
                says: "`-L uvm`",
                ..Fix::default()
            }),
            ..n(
                "uvm_without_library",
                "`import uvm_pkg::*` without `-L uvm`",
                "top.sv",
                "xvlog",
                e,
                Some("VRFC 10-2989"),
                2,
                "uvm_pkg",
            )
        },
        n(
            "dpi_import_without_library",
            "DPI-C import with no C library linked (`-sv_lib` is not supported for designs)",
            "top.sv",
            "xelab",
            e,
            None,
            3,
            "c_add",
        ),
        n(
            "encrypted_envelope",
            "IEEE-1735 `pragma protect` envelope without a vendor key",
            "top.sv",
            "-",
            DiagSeverity::Critical,
            None,
            3,
            "pragma protect",
        ),
        Neg {
            fix: Some(Fix {
                lang: Some(Lang::Vhdl2008),
                says: "`xvhdl --2008` (`vhdl2008` in a project file)",
                ..Fix::default()
            }),
            ..n(
                "vhdl_2008_construct",
                "VHDL-2008 `process (all)` compiled as VHDL-93",
                "top.vhd",
                "xvhdl",
                e,
                Some("VRFC 10-1449"),
                8,
                "process (all)",
            )
        },
        n(
            "nettype",
            "SystemVerilog user-defined `nettype`",
            "top.sv",
            "xelab",
            e,
            Some("XSIM 43-3980"),
            4,
            "nettype",
        ),
        n(
            "unknown_system_task",
            "system task from a VPI library that is not loaded",
            "top.sv",
            "xelab",
            e,
            Some("XSIM 43-4287"),
            4,
            "$my_vpi_hello",
        ),
        n(
            "generic_interconnect",
            "SystemVerilog `interconnect` net type",
            "top.sv",
            "xelab",
            e,
            Some("VRFC 10-98"),
            4,
            "interconnect",
        ),
        n(
            "syntax_error",
            "plain syntax error",
            "top.sv",
            "xvlog",
            e,
            Some("VRFC 10-4982"),
            5,
            ";",
        ),
        n(
            "undeclared_identifier",
            "assignment to an undeclared identifier",
            "top.sv",
            "xvlog",
            e,
            Some("VRFC 10-2989"),
            4,
            "b",
        ),
        n(
            "vhdl_syntax_error",
            "VHDL syntax error (missing `;`)",
            "top.vhd",
            "xvhdl",
            e,
            Some("VRFC 10-4982"),
            7,
            "wait",
        ),
        Neg {
            fix: Some(Fix {
                xvlog: &["--relax"],
                says: "`xvlog --relax`",
                ..Fix::default()
            }),
            ..n(
                "use_before_declaration",
                "net used before its declaration (tolerated by Icarus Verilog)",
                "top.v",
                "xvlog",
                e,
                Some("VRFC 10-3380"),
                5,
                "flag",
            )
        },
        Neg {
            fix: Some(Fix {
                xvlog: &["--relax"],
                says: "`xvlog --relax`",
                ..Fix::default()
            }),
            ..n(
                "empty_statement_after_label",
                "stray `;` after a block label, `begin : name;`",
                "top.v",
                "xvlog",
                e,
                Some("VRFC 10-2096"),
                4,
                "begin : blk;",
            )
        },
        n(
            "runtime_fatal",
            "`$fatal` during the run",
            "top.sv",
            "-",
            DiagSeverity::Fatal,
            None,
            4,
            "deliberate failure",
        ),
        Neg {
            line_slack: 1,
            ..n(
                "readmem_missing_file",
                "`$readmemh` of a file that is not there (warning only)",
                "top.sv",
                "-",
                DiagSeverity::Warning,
                None,
                4,
                "no_such_file.hex",
            )
        },
    ]
}

// ------------------------------------------------------------------------------------------
// Environment
// ------------------------------------------------------------------------------------------

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn corpus_dir() -> PathBuf {
    workspace().join("tests").join("rtl")
}

fn find_vivado() -> Option<PathBuf> {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return None;
    }
    if let Some(p) = std::env::var_os("ZYNQSIM_VIVADO") {
        return Some(PathBuf::from(p));
    }
    let root = PathBuf::from(r"C:\Xilinx\Vivado");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("settings64.bat").is_file())
        .collect();
    versions.sort();
    versions.pop()
}

fn work_base() -> PathBuf {
    let base = std::env::var_os("ZYNQSIM_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("corpus-{}", std::process::id()))
}

/// At most this many XSim builds at once: each starts several processes and the machine also runs
/// other tests.
const PARALLEL: usize = 3;

struct Gate {
    free: Mutex<usize>,
    cv: Condvar,
}

fn gate() -> &'static Gate {
    static G: OnceLock<Gate> = OnceLock::new();
    G.get_or_init(|| Gate {
        free: Mutex::new(PARALLEL),
        cv: Condvar::new(),
    })
}

struct Permit;

impl Permit {
    fn acquire() -> Permit {
        let g = gate();
        let mut free = g.free.lock().unwrap();
        while *free == 0 {
            free = g.cv.wait(free).unwrap();
        }
        *free -= 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let g = gate();
        *g.free.lock().unwrap() += 1;
        g.cv.notify_one();
    }
}

/// Per-case memo cells, keyed by case name.
type MemoStore<T> = OnceLock<Mutex<HashMap<String, Arc<OnceLock<T>>>>>;

/// One run per case per test process, shared by the case's own test and the table test.
fn memo<T: Clone + Send + Sync + 'static>(
    store: &'static MemoStore<T>,
    key: &str,
    make: impl FnOnce() -> T,
) -> T {
    let cell = {
        let mut m = store
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        m.entry(key.to_string()).or_default().clone()
    };
    cell.get_or_init(|| {
        let _p = Permit::acquire();
        make()
    })
    .clone()
}

fn count_lines(base: &Path, files: &[&str]) -> usize {
    files
        .iter()
        .map(|f| {
            std::fs::read_to_string(base.join(f))
                .map(|t| t.lines().count())
                .unwrap_or(0)
        })
        .sum()
}

// ------------------------------------------------------------------------------------------
// Running
// ------------------------------------------------------------------------------------------

/// The outcome of one positive case, reduced to what the table shows.
#[derive(Clone)]
struct Row {
    compiled: bool,
    elaborated: bool,
    ran: bool,
    passed: bool,
    warnings: usize,
    /// `pass`, or the stage and the first error's file:line and construct.
    summary: String,
    wall: Duration,
    report: Arc<StandaloneReport>,
}

/// A path as the tables show it: relative to `tests/rtl`, forward slashes.
fn short(file: &str) -> String {
    let c = corpus_dir().display().to_string().replace('\\', "/");
    let c = c.trim_start_matches(r"\\?\").to_string();
    // The manifest paths contain `..`; compare on the tail instead.
    if let Some(i) = file.find("/tests/rtl/") {
        return file[i + "/tests/rtl/".len()..].to_string();
    }
    if let Some(i) = file.find("/fixtures/") {
        return file[i + 1..].to_string();
    }
    file.strip_prefix(&c).unwrap_or(file).to_string()
}

fn first_error(report: &StandaloneReport) -> String {
    match report.errors().next() {
        Some(d) => {
            let place = match (&d.file, d.line) {
                (Some(f), Some(l)) => format!("{}:{}", short(f), l),
                _ => "-".to_string(),
            };
            let code = d.code.as_deref().unwrap_or("");
            format!("{place} `{}` {code}", d.construct_or_message())
                .trim()
                .to_string()
        }
        None => String::new(),
    }
}

fn passed(d: &Design, r: &StandaloneReport) -> bool {
    if !(r.compiled && r.elaborated && r.ran && r.finish.is_some()) {
        return false;
    }
    match d.pass {
        Pass::Marker => r.printed(&format!("CORPUS_PASS {}", d.name)) && !r.printed("CORPUS_FAIL"),
        Pass::Upstream => {
            r.printed("test cases completed successfully")
                && !r.printed("did not complete successfully")
        }
    }
}

fn run_design(d: &Design) -> Row {
    let vivado = find_vivado().expect("Vivado is installed (checked by the caller)");
    let base = corpus_dir().join(d.name);
    let work = work_base().join(d.name);
    let mut cfg = StandaloneConfig::new(vivado, d.top, work.clone());
    let lang_of = |p: &str| {
        let path = base.join(p);
        let l = Lang::from_path(&path).unwrap_or(Lang::SystemVerilog);
        let l = if d.sv && l == Lang::Verilog {
            Lang::SystemVerilog
        } else {
            l
        };
        StandaloneSource::auto(path).with_lang(l)
    };
    cfg.sources = d.rtl.iter().chain(d.tb).map(|p| lang_of(p)).collect();
    cfg.include_dirs = d.includes.iter().map(|i| base.join(i)).collect();
    cfg.defines = d.defines.iter().map(|s| s.to_string()).collect();
    cfg.libs = d.libs.iter().map(|s| s.to_string()).collect();
    cfg.extra_tops = d.extra_tops.iter().map(|s| s.to_string()).collect();
    cfg.xvlog_args = d.xvlog.iter().map(|s| s.to_string()).collect();
    cfg.xelab_args = d.xelab.iter().map(|s| s.to_string()).collect();
    let report = run_standalone(&cfg).expect("standalone run");
    let ok = passed(d, &report);
    let stage = report.failed_stage();
    let summary = if ok {
        "pass".to_string()
    } else if stage != "-" {
        format!("{stage}: {}", first_error(&report))
    } else if report.timed_out {
        "timed out".to_string()
    } else if report.errors().next().is_some() {
        format!("run: {}", first_error(&report))
    } else {
        "ran, self-check did not pass".to_string()
    };
    let warnings = report
        .diagnostics
        .iter()
        .filter(|x| x.severity == DiagSeverity::Warning || x.severity == DiagSeverity::Critical)
        .count();
    if ok {
        let _ = std::fs::remove_dir_all(&work);
    }
    Row {
        compiled: report.compiled,
        elaborated: report.elaborated,
        ran: report.ran,
        passed: ok,
        warnings,
        summary,
        wall: report.wall,
        report: Arc::new(report),
    }
}

fn design_row(d: &'static Design) -> Row {
    static S: MemoStore<Row> = OnceLock::new();
    memo(&S, d.name, || run_design(d))
}

/// A probe is a design of one file `tests/rtl/<name>/top.sv` printing `CORPUS_PASS <name>`.
fn probe_design(name: &'static str, libs: &'static [&'static str]) -> Design {
    Design {
        name,
        upstream: "-",
        commit: "-",
        licence: "Apache-2.0 (this repository)",
        rtl: &[],
        tb: &["top.sv"],
        top: "top",
        libs,
        ..D
    }
}

fn probe_row(name: &'static str, libs: &'static [&'static str]) -> Row {
    static S: MemoStore<Row> = OnceLock::new();
    memo(&S, name, || run_design(&probe_design(name, libs)))
}

fn probe_libs(name: &str) -> &'static [&'static str] {
    if name == UVM_PROBE.0 { &["uvm"] } else { &[] }
}

/// Result of a negative case (and of its fix).
#[derive(Clone)]
struct NegRow {
    report: Arc<StandaloneReport>,
    /// The report with the fix applied.
    fixed: Option<Arc<StandaloneReport>>,
    wall: Duration,
}

fn run_neg_config(n: &Neg, fix: Option<&Fix>, vivado: &Path, work_tag: &str) -> StandaloneReport {
    let base = corpus_dir().join("negative").join(n.name);
    let work = work_base().join(format!("neg-{}-{work_tag}", n.name));
    let mut cfg = StandaloneConfig::new(vivado.to_path_buf(), n.top, work);
    let mut source = StandaloneSource::auto(base.join(n.file));
    if let Some(f) = fix {
        if let Some(l) = f.lang {
            source = source.with_lang(l);
        }
        cfg.defines = f.defines.iter().map(|s| s.to_string()).collect();
        cfg.libs = f.libs.iter().map(|s| s.to_string()).collect();
        cfg.include_dirs = f.includes.iter().map(|i| base.join(i)).collect();
        cfg.extra_tops = f.extra_tops.iter().map(|s| s.to_string()).collect();
        cfg.xvlog_args = f.xvlog.iter().map(|s| s.to_string()).collect();
    }
    cfg.sources = vec![];
    if let Some(f) = fix {
        for v in f.vivado_sources {
            cfg.sources
                .push(StandaloneSource::auto(vivado.join(v)).with_lang(Lang::Verilog));
        }
    }
    cfg.sources.push(source);
    run_standalone(&cfg).expect("standalone run")
}

fn neg_row(n: &'static Neg) -> NegRow {
    static S: MemoStore<NegRow> = OnceLock::new();
    memo(&S, n.name, || {
        let vivado = find_vivado().expect("Vivado is installed (checked by the caller)");
        let report = run_neg_config(n, None, &vivado, "bad");
        let fixed = n
            .fix
            .as_ref()
            .map(|f| Arc::new(run_neg_config(n, Some(f), &vivado, "fixed")));
        let wall = report.wall + fixed.as_ref().map_or(Duration::ZERO, |f| f.wall);
        NegRow {
            report: Arc::new(report),
            fixed,
            wall,
        }
    })
}

fn negs() -> &'static [Neg] {
    static N: OnceLock<Vec<Neg>> = OnceLock::new();
    N.get_or_init(negatives)
}

// ------------------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------------------

macro_rules! need_vivado {
    () => {
        if find_vivado().is_none() {
            eprintln!("SKIPPED: Vivado/XSim not found");
            return;
        }
    };
}

fn assert_design(d: &'static Design) {
    let row = design_row(d);
    let tail: String = row
        .report
        .sim_output
        .lines()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    match d.expect {
        Expect::Pass => assert!(
            row.passed,
            "{}: {}\n--- last output lines ---\n{tail}\nwork dir: {}",
            d.name,
            row.summary,
            row.report.work_dir.display()
        ),
        Expect::Unsupported(why) => {
            assert!(!row.passed, "{} was expected not to pass", d.name);
            assert!(
                row.summary.contains(why),
                "{}: summary `{}` does not mention `{why}`",
                d.name,
                row.summary
            );
        }
    }
}

macro_rules! corpus_tests {
    ($($test:ident => $idx:expr,)*) => {
        $(
            #[test]
            fn $test() {
                need_vivado!();
                assert_design(&DESIGNS[$idx]);
            }
        )*
    };
}

corpus_tests! {
    corpus_secworks_sha256 => 0,
    corpus_secworks_sha1 => 1,
    corpus_secworks_md5 => 2,
    corpus_secworks_aes => 3,
    corpus_secworks_chacha => 4,
    corpus_forencich_lfsr => 5,
    corpus_forencich_uart => 6,
    corpus_forencich_axis => 7,
    corpus_picorv32 => 8,
    corpus_uriscv => 9,
    corpus_pulp_common_cells => 10,
    corpus_serv => 11,
    corpus_dpretet_async_fifo => 12,
}

#[test]
fn corpus_vivado_funcsim_netlist() {
    need_vivado!();
    assert_design(&FUNCSIM);
}

#[test]
fn the_manifest_matches_the_files_on_disk() {
    // No XSim needed: every file named exists, has a licence and a SOURCE.md, and the directory
    // holds no stray design file the manifest does not know.
    let base = corpus_dir();
    let mut third_party = 0;
    for d in DESIGNS {
        let dir = base.join(d.name);
        for f in d.rtl.iter().chain(d.tb) {
            assert!(dir.join(f).is_file(), "{}: missing {f}", d.name);
        }
        let lic = std::fs::read_to_string(dir.join("LICENSE")).expect("LICENSE");
        assert!(lic.len() > 400, "{}: LICENSE is too short", d.name);
        let src = std::fs::read_to_string(dir.join("SOURCE.md")).expect("SOURCE.md");
        assert!(
            src.contains(d.commit),
            "{}: SOURCE.md lacks the commit",
            d.name
        );
        assert!(
            src.contains(d.upstream),
            "{}: SOURCE.md lacks the URL",
            d.name
        );
        assert_eq!(d.commit.len(), 40, "{}: commit is a full hash", d.name);
        third_party += 1;
    }
    assert!(
        third_party >= 10,
        "the corpus needs at least 10 third-party designs"
    );
    for (name, _) in PROBES.iter().chain([&UVM_PROBE]) {
        assert!(base.join(name).join("top.sv").is_file(), "{name}");
    }
    for n in negs() {
        assert!(
            base.join("negative").join(n.name).join(n.file).is_file(),
            "{}",
            n.name
        );
    }
    let on_disk: usize = std::fs::read_dir(&base)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .count();
    // designs + funcsim + probes + the `negative` folder
    assert_eq!(
        on_disk,
        DESIGNS.len() + 1 + PROBES.len() + 1 + 1,
        "tests/rtl holds a folder the manifest does not list"
    );
}

macro_rules! probe_tests {
    ($($test:ident => $name:expr,)*) => {
        $(
            #[test]
            fn $test() {
                need_vivado!();
                let row = probe_row($name, probe_libs($name));
                assert!(
                    row.passed,
                    "{}: {}\n{}",
                    $name,
                    row.summary,
                    row.report.sim_output
                );
            }
        )*
    };
}

probe_tests! {
    probe_bind => "probe-bind",
    probe_virtual_modport => "probe-virtual-modport",
    probe_config_block => "probe-config-block",
    probe_covergroup => "probe-covergroup",
    probe_constrained_random => "probe-constrained-random",
    probe_concurrent_assertion => "probe-concurrent-assertion",
    probe_uvm_with_library => "probe-uvm-with-library",
}

/// The checks every negative case shares: the tool failed where expected and the diagnostic names
/// file, line and construct. The fix, when there is one, makes the same source pass.
fn assert_negative(name: &str) {
    let n = negs().iter().find(|n| n.name == name).expect("case");
    let row = neg_row(n);
    let r = &row.report;
    assert_eq!(
        r.failed_stage(),
        n.stage,
        "{name}: failed at the wrong stage\n{:#?}",
        r.diagnostics
    );
    let hit = r.diagnostics.iter().find(|d| {
        d.severity == n.severity
            && d.file_name() == Some(n.file)
            && n.line_ok(d.line)
            && d.construct.contains(n.construct)
            && n.code.is_none_or(|c| d.code.as_deref() == Some(c))
    });
    let Some(hit) = hit else {
        panic!(
            "{name}: no {} diagnostic at {}:{} naming `{}`{}\n{:#?}",
            n.severity,
            n.file,
            n.line,
            n.construct,
            n.code
                .map(|c| format!(" with code {c}"))
                .unwrap_or_default(),
            r.diagnostics
        );
    };
    // The text a user sees carries the same three facts.
    let shown = hit.to_string();
    assert!(
        shown.contains(&format!("{}:{}:", n.file, hit.line.unwrap_or(0)))
            && shown.contains(n.construct),
        "{name}: rendered `{shown}`"
    );
    // A negative case never passes by accident (the warning-only case prints its marker).
    if n.severity >= DiagSeverity::Error {
        assert!(
            !r.printed(&format!("CORPUS_PASS {name}")),
            "{name}: the design ran to its pass marker"
        );
    }
    if let (Some(fix), Some(fixed)) = (&n.fix, &row.fixed) {
        assert!(
            fixed.compiled && fixed.elaborated && fixed.ran,
            "{name}: with {} the build still fails\n{:#?}",
            fix.says,
            fixed.diagnostics
        );
        assert!(
            fixed.printed(&format!("CORPUS_PASS {name}")),
            "{name}: with {} the design does not pass\n{}",
            fix.says,
            fixed.sim_output
        );
    }
}

macro_rules! negative_tests {
    ($($test:ident => $name:expr,)*) => {
        $(
            #[test]
            fn $test() {
                need_vivado!();
                assert_negative($name);
            }
        )*
    };
}

negative_tests! {
    negative_missing_include => "missing_include",
    negative_missing_define => "missing_define",
    negative_missing_unisim_library => "missing_unisim_library",
    negative_missing_module => "missing_module",
    negative_uvm_without_library => "uvm_without_library",
    negative_dpi_import_without_library => "dpi_import_without_library",
    negative_encrypted_envelope => "encrypted_envelope",
    negative_vhdl_2008_construct => "vhdl_2008_construct",
    negative_nettype => "nettype",
    negative_unknown_system_task => "unknown_system_task",
    negative_generic_interconnect => "generic_interconnect",
    negative_syntax_error => "syntax_error",
    negative_undeclared_identifier => "undeclared_identifier",
    negative_vhdl_syntax_error => "vhdl_syntax_error",
    negative_use_before_declaration => "use_before_declaration",
    negative_empty_statement_after_label => "empty_statement_after_label",
    negative_runtime_fatal => "runtime_fatal",
    negative_readmem_missing_file => "readmem_missing_file",
}

#[test]
fn every_negative_case_has_a_test() {
    // The macro list above and the manifest must not drift apart.
    let src =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus.rs"))
            .unwrap();
    for n in negs() {
        assert!(
            src.contains(&format!("=> \"{}\",", n.name)),
            "no negative test for {}",
            n.name
        );
    }
}

// ------------------------------------------------------------------------------------------
// docs/CORPUS.md
// ------------------------------------------------------------------------------------------

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn render_designs(rows: &[(&Design, Row)]) -> String {
    let mut s = String::new();
    s.push_str(
        "| Design | Licence | Commit | Lines (rtl + tb) | Compiled | Elaborated | Ran | \
         Self-check | Warnings | Result / notes |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for (d, r) in rows {
        let dir = corpus_dir().join(d.name);
        let commit = if d.commit.len() == 40 {
            format!("`{}`", &d.commit[..12])
        } else {
            d.commit.to_string()
        };
        let note = if r.passed {
            d.notes.to_string()
        } else {
            format!("{} ({})", r.summary, d.notes)
        };
        let _ = writeln!(
            s,
            "| [{n}]({n}/SOURCE.md) | {lic} | {commit} | {rtl} + {tb} | {c} | {e} | {ran} | {p} | {w} | {note} |",
            n = d.name,
            lic = cell(d.licence),
            rtl = count_lines(&dir, d.rtl),
            tb = count_lines(&dir, d.tb),
            c = yes(r.compiled),
            e = yes(r.elaborated),
            ran = yes(r.ran),
            p = if r.passed { "pass" } else { "FAIL" },
            w = r.warnings,
            note = cell(&note),
        );
    }
    s
}

fn render_probes(rows: &[(&str, &str, Row)]) -> String {
    let mut s = String::new();
    s.push_str("| Probe | Construct | Compiled | Ran | Result |\n|---|---|---|---|---|\n");
    for (name, what, r) in rows {
        let _ = writeln!(
            s,
            "| [{name}]({name}/top.sv) | {} | {} | {} | {} |",
            cell(what),
            yes(r.compiled),
            yes(r.ran),
            if r.passed {
                "pass".to_string()
            } else {
                cell(&r.summary)
            }
        );
    }
    s
}

fn render_negatives(rows: &[(&Neg, NegRow)]) -> String {
    let mut s = String::new();
    s.push_str(
        "| Case | Unsupported construct | Stops at | Diagnostic (code, file:line, construct) | \
         Passes with |\n|---|---|---|---|---|\n",
    );
    for (n, r) in rows {
        let hit = r
            .report
            .diagnostics
            .iter()
            .find(|d| {
                d.severity == n.severity
                    && d.file_name() == Some(n.file)
                    && n.line_ok(d.line)
                    && d.construct.contains(n.construct)
            })
            .map(|d| {
                format!(
                    "{} {}:{} `{}`",
                    d.code.as_deref().unwrap_or(&d.severity.to_string()),
                    n.file,
                    n.line,
                    d.construct_or_message()
                )
            })
            .unwrap_or_else(|| "(not found)".to_string());
        let stage = match n.stage {
            "-" => match n.severity {
                DiagSeverity::Fatal => "run (fatal)",
                DiagSeverity::Warning => "run (warning)",
                _ => "runs, design silently empty",
            },
            other => other,
        };
        let fix = n.fix.as_ref().map_or("-", |f| f.says);
        let _ = writeln!(
            s,
            "| [{n}](negative/{n}/{f}) | {what} | {stage} | {hit} | {fix} |",
            n = n.name,
            f = n.file,
            what = cell(n.what),
            hit = cell(&hit),
            fix = cell(fix),
        );
    }
    s
}

fn render_timings(
    designs: &[(&Design, Row)],
    probes: &[(&str, &str, Row)],
    negs: &[(&Neg, NegRow)],
) -> String {
    let mut s = String::new();
    s.push_str("| Case | Wall time (s) |\n|---|---|\n");
    for (d, r) in designs {
        let _ = writeln!(s, "| {} | {:.1} |", d.name, r.wall.as_secs_f64());
    }
    for (n, _, r) in probes {
        let _ = writeln!(s, "| {n} | {:.1} |", r.wall.as_secs_f64());
    }
    for (n, r) in negs {
        let _ = writeln!(s, "| negative/{} | {:.1} |", n.name, r.wall.as_secs_f64());
    }
    s
}

/// Replaces the text between `<!-- BEGIN:name -->` and `<!-- END:name -->`.
fn block(doc: &str, name: &str) -> Option<(usize, usize)> {
    let begin = format!("<!-- BEGIN:{name} -->\n");
    let end = format!("<!-- END:{name} -->");
    let a = doc.find(&begin)? + begin.len();
    let b = doc[a..].find(&end)? + a;
    Some((a, b))
}

#[test]
fn corpus_table_is_current() {
    need_vivado!();
    let designs: Vec<(&Design, Row)> = DESIGNS
        .iter()
        .chain([&FUNCSIM])
        .map(|d| (d, design_row(d)))
        .collect();
    let probes: Vec<(&str, &str, Row)> = PROBES
        .iter()
        .chain([&UVM_PROBE])
        .map(|(n, w)| (*n, *w, probe_row(n, probe_libs(n))))
        .collect();
    let negatives: Vec<(&Neg, NegRow)> = negs().iter().map(|n| (n, neg_row(n))).collect();

    let generated = [
        ("designs", render_designs(&designs)),
        ("probes", render_probes(&probes)),
        ("negatives", render_negatives(&negatives)),
    ];
    let timings = render_timings(&designs, &probes, &negatives);

    let path = workspace().join("docs").join("CORPUS.md");
    let mut doc = std::fs::read_to_string(&path).expect("docs/CORPUS.md exists");
    let bless = std::env::var_os("ZYNQSIM_CORPUS_BLESS").is_some();
    let mut stale = Vec::new();
    for (name, text) in generated
        .iter()
        .map(|(n, t)| (*n, t.as_str()))
        .chain([("timings", timings.as_str())])
    {
        let (a, b) = block(&doc, name).unwrap_or_else(|| panic!("CORPUS.md lacks block {name}"));
        if doc[a..b] != *text {
            // Wall times are informational: never compared.
            if name != "timings" {
                stale.push(name);
            }
            if bless {
                doc.replace_range(a..b, text);
            }
        }
    }
    if bless {
        std::fs::write(&path, doc).unwrap();
        eprintln!("docs/CORPUS.md rewritten");
        return;
    }
    assert!(
        stale.is_empty(),
        "docs/CORPUS.md is stale in {stale:?}: run `cargo xtask corpus` and review the diff"
    );
}
