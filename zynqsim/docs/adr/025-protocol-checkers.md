# ADR-025: AXI protocol checkers in the generated testbench

Status: **Accepted** (2026-10-16). On by default; `[rtl] protocol_checkers` switches them.

## Context

Prompt §9.4 asks for protocol checkers (VALID/READY rules, 4 KB crossing, WLAST, alignment) that are
on by default and report with timestamps; §14.4 asks for tests with valid and deliberately illegal
behaviour. Until now `docs/STATUS.md` said "stub". The generated testbench (`zynqsim init --ip`) drives
the IP's AXI4-Lite slave and answers its AXI4 master, but it asserted nothing about the handshakes.

## Decision

1. **Checkers are SystemVerilog observers.** `fixtures/rtl/common/zs_axi_checker.sv` defines
   `zs_axi_checker #(ID, FULL, AW, DW)`. It drives no signal. `init --ip` writes it next to the
   testbench (`tb/zs_axi_checker.sv`, listed in `project.toml`, embedded in the CLI with
   `include_str!`) and instantiates it twice: `ID 0` on the AXI4-Lite slave the testbench drives
   (VALID/READY, reset, responses), `ID 1` on the AXI4 master the IP drives (the same plus bursts:
   4 KB, WLAST/RLAST, WRAP). Plain `always` blocks, `$time`, one DPI call; no assertions, so XSim's
   SVA support is not needed.
2. **One host path.** The checker calls the DPI function `zs_protocol_check`, which sends one
   interrupt packet on remote-port device 21 (`pl_api::protocol::DEV_PROTOCOL_CHECK`). The XSim
   backend collects the packets and the QEMU engine turns each into a trace marker
   `protocol check <warning|error|info> at <t> ns: <port> <channel> <RULE> detail=0x..`. The CLI,
   MCP and GUI already show trace markers, so nothing else needed a new record type.
3. **Findings before the connection are kept.** The reset comes before `rp_open`; the DPI keeps up to
   256 findings while disconnected and sends them, with their own timestamps, after the hello.
4. **Severity is a project setting.** `[rtl] protocol_checkers = true` (default) gives warnings;
   `false` (or `"off"`) defines `ZS_NO_PROTOCOL_CHECKERS`, which compiles the instances out of the
   testbench (the define is part of the build key); `"strict"` makes a violation an `error` marker
   and `zynqsim run` exits 1. Informational findings (SLVERR/DECERR) are never failures.
5. **Rules** (codes in `pl_api::protocol`, mirrored in the SV header):
   VALID_DROP, PAYLOAD_UNSTABLE (both per channel, AW/W/B/AR/R), RESET_VALID (VALID high from the
   second cycle of ARESETN low), CROSS_4KB (INCR), LAST_EARLY / LAST_MISSING (W and R), WRAP_ALIGN,
   WRAP_LEN, SIZE (AxSIZE wider than the bus), RESP (info).

## Limits

* WLAST/RLAST are checked against the address queue. A W beat that arrives before any AW turns the
  WLAST check off for that port for the rest of the run (AXI allows the order; the bridge never uses
  it). Read beats are checked against accepted ARs only.
* INCR bursts with unaligned starts are legal and not reported; only WRAP alignment is checked.
* Hand-written testbenches (`fixtures/rtl/*/cosim_top.sv`, the examples' own testbenches) do not
  instantiate the checkers; only `init --ip` and `init --export-sim` testbenches do.
* The checkers see the bus at the testbench, not inside the design. Findings are timed in the XSim
  time base (1 ns resolution).
* The Rust side of the rule table and the SV side are kept in step by hand; the test
  `sim-api::tbgen::the_checker_source_names_every_rule_the_host_knows` catches a missing name.

## Consequences

* Generated testbenches cost a few more XSim instances; the trace digest of a design with no
  findings does not change (no markers), so recorded sessions of examples are unaffected.
* Tests: `pl-api::protocol`, `xsim-dpi::core` (packet encoding, early findings), `sim-api::protocol`
  and `sim-api::tbgen` (instantiation, rule names), `sim-api::project` (the setting), and
  `zynqsim::protocol_checks` (legal designs produce none; five illegal fixtures in
  `fixtures/rtl/illegal/` produce the expected rule at the expected time; strict and off).
