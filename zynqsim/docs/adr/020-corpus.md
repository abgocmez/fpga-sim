# ADR-020: Universality corpus

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

The prompt's §14 item 5 asks for at least ten third-party designs run through the RTL backend.
Until now every RTL test used this project's own designs or Vivado's export. We need evidence
that real, unmodified-in-function RTL from other authors compiles and runs in XSim, and an
honest list of what does not.

## Decision

* **Designs live in `tests/rtl/<name>/`** with `rtl/`, `tb/` (upstream testbench where one
  exists, otherwise a testbench that prints `CORPUS_PASS`), a `LICENSE` copied verbatim from
  upstream, and `SOURCE.md` with the upstream URL, the full commit hash and the licence name.
  Only permissive licences are accepted (MIT, ISC, BSD, Apache-2.0, Solderpad 0.51). GPL/LGPL
  or unclear licences are removed, not vendored.
* **Driver**: `crates/backend-rtl-xsim/tests/corpus.rs` with the standalone XSim flow in
  `src/standalone.rs` (compile, elaborate, run, classify diagnostics by file, line and
  construct). One test per design, per negative case and per probe; a test runs each case once
  per process and shares the result with the table test.
* **Expectations are per case**: a design is `Pass` (compiles, elaborates, runs, and the upstream
  or marker self-check passes), or `Unsupported(reason)` where the test asserts the reason appears
  in the summary. The manifest test checks licence, commit and source files for every design.
* **Table**: `docs/CORPUS.md` is generated from the results. `corpus_table_is_current` fails when
  the file is stale. Regenerate with `ZYNQSIM_CORPUS_BLESS=1` on the test (there is no
  `cargo xtask corpus` subcommand). Wall times are shown but never compared.
* **Negative cases** (`tests/rtl/negative/`) and **probes** (`tests/rtl/probe-*`) are not
  designs: a negative case must fail at the stated stage with its diagnostic, and a probe
  records whether a construct runs.

## Result

Thirteen third-party designs pass (five secworks cores, three forencich modules, picorv32, serv,
uriscv, pulp common_cells, dpretet async FIFO), together with one design from this repository. No
third-party design is `unsupported`. Eighteen negative cases fail as expected, and seven probes run.

## Not done, and said by the table

* No design was run with timing (SDF) simulation or against Vivado IP libraries.
* UVM is probed with the library present only; no full UVM testbench is claimed.
* Each design runs once with the stated check; this is not a conformance suite for the designs.
* The corpus needs Vivado/XSim and skips with a message when it is absent, so it is not run in
  CI on machines without Vivado.
