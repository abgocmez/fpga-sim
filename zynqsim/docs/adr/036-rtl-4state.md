# ADR-036: Four-state values in the native RTL engine

Status: **Accepted**. Date: 2026-10-17. Supersedes the 2-state decision in ADR-034 (its
"Values" section and the 2-state limit); the rest of ADR-034 stands.

## Context

ADR-034 kept the engine 2-state: x and z read as 0. That made the engine decide some designs
differently from XSim (any testbench that depends on x propagation or on `===` against x), and it
made an uninitialised `reg` start at 0 where Verilog starts it at X.

## Decision

1. **Representation** (`crates/rtl-engine/src/value.rs`, `Value4`): three masks per value, `bits`
   (known ones), `unknown` (X or Z) and `z` (a subset of `unknown`), normalised to the width. Up to
   128 bits, as before.
2. **Operators** follow IEEE 1364-2005 clause 5: bitwise tables (0 dominates `&`, 1 dominates `|`);
   arithmetic is all X on any unknown operand bit, and division by zero is X; `==`/`!=` give 0 when a
   bit known on both sides differs, else X when an operand has an unknown bit (XSim agrees; found by the
   differential); the relational operators are X on any unknown operand bit; shifts move Z as they move
   other bits (XSim agrees); `===`/`!==` are exact; `&&`, `||`, `!`
   use truth values; reductions fold the bit tables; shifts by an unknown amount are X; `?:` with an
   unknown condition merges the branches bitwise. Operators turn Z into X; selects, concatenation
   and the conditional merge keep Z.
3. **Storage class** (`SigKind`): a net (`wire` and ports without a type) is Z until driven. Each
   continuous assignment is a driver of the bits it writes; the net value is the resolution of its
   drivers (Z loses, equal known values keep, a conflict or an X gives X). A value set from outside
   (`Simulator::set4`) is an external driver of the net. A `reg`, `logic`, `integer` or function local is
   X until assigned. A `bit` or `int` starts at 0 and X or Z written to it becomes 0.
4. **Control**: `if`, `while`, `wait` and `for` conditions take the else branch on X
   (`truth() == Some(true)` to proceed). `case` items compare with `===`; `casez` treats Z on either
   side as don't-care; `casex` treats X and Z on either side as don't-care.
5. **Edges**: posedge is 0 to 1, 0 to X or Z, X or Z to 1; negedge is the dual. Scheduling is unchanged.
6. **Interfaces**: `Simulator::get`/`set` keep the 2-state view (X and Z read as 0 in `get`);
   `get4`, `set4`, `get_word4`, `set_word4` carry `Value4`. `state_hash` and `snapshot` include the
   masks (snapshot magic `RTLENG02`). `Change` carries the 4-state value. `$display` prints x and z
   digits (`%b` per bit; `%h`/`%o` per digit group: all x `x`, all z `z`, partly unknown `X` when an
   x is present and `Z` otherwise); VCD writes `x` and `z`.

## Verification

* `tests/fourstate.rs` (30): every operator class against reference tables written in the test from
  IEEE 1364 (bitwise, unary and logical, reductions, equality, relational, arithmetic, shifts,
  conditional, concatenation and selects, literals); the resolution table for two and three drivers
  of a tri-state net; `if` on X; casez and casex; posedge and negedge with X transitions; a counter
  without reset that stays X until reset; initial blocks; `bit` coercion; display formats; VCD; the
  2-state view and `set4`; snapshot and hash; a property that known inputs give the 2-state results
  (500 random operand sets); a soundness property (for random operands with unknown bits, every known
  result bit equals its value under every completion of the unknown bits).
* `scheduling` (11): the combinational-loop tests were adjusted: an X-valued ring is a fixed point
  (no oscillation) and a loop needs three inverters with a toggling input.
* `corpus` (6 of 9 pass, unchanged) and `xsim_diff` (`my_math_ip` against XSim) still pass.

## Consequences

* Engine results now differ from ADR-034's 2-state results only where X or Z appear (uninitialised
  variables, undriven nets, x/z literals, unknown operands). Designs without them are unchanged.
* Still not modelled: drive strength, `wand`/`wor` (plain nets), `inout` ports (refused).
* Not yet compared with XSim on a broad X-propagation set; the planned differential testbench is in
  `crates/rtl-engine/tests/xprop/xprop_tb.v`.
* Differential vs XSim: `tests/xprop_diff.rs` runs `tests/xprop/xprop_tb.v` (about 35 checks of operators,
  a tri-state bus, the counter without reset, casez and `if` on x) in both. The first run found two
  differences (`==` with a known differing bit; shifts of z), both fixed in the engine.
