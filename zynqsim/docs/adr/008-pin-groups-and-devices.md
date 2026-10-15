# ADR-008: Pmod and RGB pins travel as pin groups; devices are host models

Status: **Accepted**. Date: 2026-10-15.

## Context

The board's Pmod ports and RGB LEDs are PL pins. A Pmod module (an OLED, a sensor) speaks a
protocol on them (SPI, I2C) in simulated time. The host wants the *module's behaviour* (a
picture, a reading), and wants it in the same deterministic stream as the rest of the system.

## Decision

* The testbench reports the eight levels of a Pmod port (groups 0..4 = JA..JE) or the six RGB LED
  pins (group 6) whenever they change, stamped with the PL time, as remote-port interrupt packets
  on device 19 (`zs_pin_out` DPI). The host can drive input groups on device 20 (`zs_pin_in`).
  This reuses the transport of the board pins (ADR-002, ADR-006); nothing new is opened.
* A device is a Rust model (`crates/devices`, trait `BoardDevice`) that consumes `(time, levels)`
  and may show a picture. It is independent of the PS and PL backends. The first model is an
  SSD1306-class 128x32 OLED behind SPI.
* The RGB LEDs are not a device: the engine averages their pin levels into PWM duty cycles over
  windows of at least 200 us, which is what `BoardState::rgb` holds.
* `zynqsim init --ip` forwards ports named like a Pmod (`pmod_ja`, `jb_o`...) or like the RGB LEDs
  (`rgb_leds`, six bits) without further instruction; `[devices] pmod_ja = "oled"` or
  `pmod_attach` binds a device to a port.

## Consequences

* Pin traffic costs trace records (one `pmod.ja` signal event per change, so a 1 MHz SPI byte is
  ~50 records) but no extra wall-time machinery.
* The PWM average is a measurement, not an exact value: a window holds a few periods, so duties
  are within a few counts (tests allow 4 of 255). After the testbench ends the last duty is kept.
* Only outputs from the PL are time-exact. Host-driven inputs are sampled by the testbench once a
  microsecond.
* The device models follow datasheets from memory and are not compared with hardware
  (`VERIFY:` in STATUS.md); pin order on the Pmod OLED is the Digilent reference's as recalled.
