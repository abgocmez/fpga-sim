# ADR-009: Bidirectional Pmod pins and the synchronous pin-exchange guarantee

Status: **Accepted**. Date: 2026-10-15 (documented 2026-10-18).

## Context

ADR-008 introduced one-way Pmod pin groups: the RTL testbench reports PL-driven outputs (`zs_pin_out`,
remote-port interrupt packets on device 19) and samples host-driven inputs once a microsecond
(`zs_pin_in`, device 20).

That works for display-only modules (the SSD1306 OLED, 8LD) and slow human inputs (buttons and
switches), where the PL never waits on a reply from the module within a serial bit time. It is not
sufficient for sensors that answer on the same bus (I2C ACK and read bytes on SDA, SPI MISO shifted
out on the falling edge of SCLK and sampled by the master on the next rising edge): if pin changes
travelled as posted interrupts and answers returned at the next quantum boundary or 1 us poll, the
master's setup/hold window on the next clock edge would already have passed in simulated time.

## Decision

* **Synchronous pin exchange (`zs_pin_xchg`).** A remote-port bus *read* of 1 byte on `DEV_PIN_OUT`
  (device 19) with address `group | (levels << 8)` reports that the PL changed the outputs of pin
  group `group` to `levels` at `now_ns` and blocks the RTL simulator until the host replies with the
  1-byte input vector that the attached device models drive back into the PL (`1` = released / pulled
  high). Because the read is answered in the same delta/instant of simulated time, **no simulated
  time passes between the PL driving a clock/data edge and the device's response appearing on the
  PL's input port**.
* **Device drive model (`BoardDevice::drive`, `PinDrive`, `devices::Bank`).** Each `BoardDevice`
  implements `drive(&self) -> PinDrive { mask, levels }` read immediately after `pins(t_ns, levels)`.
  Open-drain lines (I2C SDA/SCL) and unused pins are released (`mask` bit 0, which floats high to 1);
  active pulls set their bit in `mask` and the driven level in `levels`. `devices::Bank` holds the
  devices attached to groups `0..=5` (`JA..JE` and `cam`) and resolves the input byte in the adapter
  thread (`PinSink` in `pl-api`) the moment the packet arrives, rather than waiting for `flush()` at
  the end of the quantum.
* **Testbench generation (`tbgen.rs`).** When a module has both an output port and an input port on
  the same Pmod group (e.g. `pmod_ja_o` and `pmod_ja_i`), `detect_pin_groups` marks the pair as
  `paired = true` and generates:
  ```systemverilog
  always @(pmod_ja_o) if (connected) pmod_ja_i <= zs_pin_xchg(0, pmod_ja_o, $time);
  ```
  plus an initial exchange when `connected` rises.
* **Host-settable physical quantities (`DeviceValue`, `[device_values]`, `--device-set`, `pmod_set` /
  `pmod_state`).** Sensor models expose named quantities with range and unit (`temperature` in `degC`
  for the ADT7420 on Pmod TMP2, `light` in `counts` for the ADC081S021 on Pmod ALS, `ax`/`ay`/`az` in
  `g` for the ADXL345 on Pmod ACL). They can be set in `project.toml` (`[device_values]`), on the CLI
  (`--device-set [TIME@]JA.temperature=37.5` scheduled at an exact virtual time), in MCP
  (`pmod_set`, `pmod_state`), and via sliders in the GUI Pmod panel.

## Consequences

* **Deterministic, zero-simulated-delay serial protocols.** I2C and SPI masters in RTL see the
  slave's ACK/SDA or MISO update in the same simulated nanosecond as the SCLK/SCL transition they
  drove. Runs are reproducible across processes (`tmp2-demo` is in the 5-process determinism suite).
* **Wall-clock cost.** Every output pin transition on a paired port is a synchronous TCP round trip
  between the RTL simulator and the host adapter thread (~18 round trips per I2C byte, ~16 per SPI
  byte). At typical sensor poll rates (a few transfers per millisecond) this adds negligible wall
  time.
* **Separate input/output ports in RTL.** The generated testbench pairs an 8-bit output port and an
  8-bit input port (with open-drain lines combined as `out & in` at the pad); Verilog `inout` ports
  are not used by `tbgen`.
* **Register and protocol facts.** The ADT7420 (`tmp2`), ADC081S021 (`als`) and ADXL345 (`acl`)
  models are written from datasheets as remembered and are marked `VERIFY:` in `STATUS.md`.
