# ADR-018: BOOT.BIN boot

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

Prompt §6/§12/§15 want "a Vivado-exported design and its `BOOT.BIN` run end to end" and the
`[boot] mode = "elf" | "boot_bin"` switch. A `BOOT.BIN` is what a Zynq-7000 board boots from QSPI
or SD: BootROM reads the boot header, loads the FSBL into OCM and runs it; the FSBL runs
`ps7_init`, programs the PL bitstream and loads and starts the application. The simulator's PS
is QEMU's Zynq machine, which has no BootROM and an already configured SoC, and starts an ELF.

## Decision

* **A new pure-Rust crate `boot`** reads the image (`BootImage::parse`): boot header with its
  checksum, register-init table, image header table, image headers, partition headers (header
  checksum, attributes, destination, MD5 checksum type), MD5 checksums, classification of every
  partition (FSBL, bitstream, application), payload extraction, and a `LoadPlan`. Corruption is
  an `Issue` with field name, file offset and stored/expected values; only a file that cannot be
  interpreted at all (too short, wrong magic words, tables outside the file) is a hard `Err`.
  It never panics (seeded mutation test). `sim-api` re-exports it as `sim_api::bootbin` and adds
  the project glue, so front ends keep depending on `sim-api` only.
* **Boot = load the application partitions and start core 0 at their execution address.**
  bootgen stores an ELF as raw loadable segments, so the plan is written back as a minimal ELF32
  (one `PT_LOAD` per partition at its load address, `e_entry` = entry) in the run directory and
  handed to QEMU's `loader` like any `[cpu0] elf`. Nothing in the QEMU engine changed.
* **Entry point**: the first application partition with a non-zero execution address. Which
  partition the real FSBL hands off to is FSBL logic, not BootROM format (`VERIFY:`). Further
  application images are loaded but not started; no symbols survive bootgen.
* **Not done, and said by every report** (`bootbin::NOTES`): the FSBL is not executed (prompt §12's
  "run the FSBL as software" is **not** implemented), its register-init pairs are listed and not
  applied, the bitstream is recognised and never simulated (the PL is the project's RTL), boot-ROM
  timing and boot-mode pins are not modelled.
* **Refused with a message**: a non-zero key source in the boot header, partitions whose
  encrypted length differs from their data length, and any authentication certificate offset.
  There is no AES/HMAC/RSA and no key store.
* **`[boot] mode`**: `mode = "boot_bin"` (the prompt's spelling)
  with `file = "BOOT.BIN"`; `bootbin` is accepted as an alias. `cpu0.elf` is then not required and
  warns when present.
* **Tooling**: `zynqsim boot-info FILE [--json]` (exit 2 when the image cannot be booted),
  `zynqsim init --boot FILE --out DIR` (copies the image, writes and validates the project, refuses
  damaged images, never overwrites), MCP `boot_inspect` (an extension; not in the prompt's list).

## Format facts used (checked against bootgen 2022.1 output and `bootgen -read`)

| Where | Content |
|---|---|
| `0x20`, `0x24` | `0xAA995566`, `0x584C4E58` (bytes `XNLX`) |
| `0x28` | encryption status (0 = none). `VERIFY:` key-source values `0xA5C3C5A3` eFUSE, `0x3A5C3C5A` BBRAM (memory of UG585, no encrypted image available) |
| `0x2C` | header version `0x01010000` |
| `0x30..0x44` | FSBL offset, length, load, exec, total length, QSPI config word (`1`) |
| `0x48` | inverted sum of words `0x20..0x44` (matches `bootgen -read`) |
| `0x98`, `0x9C` | image header table offset (`0x8C0`), partition header table offset (`0xC80`) |
| `0xA0..0x8A0` | up to 256 (address, value) pairs ended by address `0xFFFFFFFF`; `[init]` fills it |
| IHT | `0x01020000`, partition count, partition header offset (words), first image header (words), authentication offset. Its word `0x3C` is `0xFFFFFFFF` in bootgen output and is not checked |
| image header | next (words), partition header (words), 0, partition count, name big-endian packed |
| partition header (`0x40`) | encrypted / unencrypted / total length (words), load, exec, data offset (words), attributes, section count, checksum offset (words), image header (words), auth cert (words), reserved, inverted sum of the 15 words before |
| attributes | bits 5:4 destination (1 PS, 2 PL, 3 INT), 14:12 checksum type (1 MD5), 17:16 owner (`VERIFY:`). Bootgen's own `-read` decodes these with the ZynqMP layout and prints `dest_device [none]`, which is wrong for Zynq-7000 |
| MD5 | 16 bytes at the checksum offset, over the unencrypted data length |

An ELF with several loadable segments becomes several partitions under one image header; only the
first carries the execution address. A bss segment with no file bytes produces no partition (checked with `rust_irq.elf`: 3 PT_LOAD, 2 partitions; QEMU's
RAM is zero).

## Consequences

* The route proves "an image bootgen made boots the same firmware as the ELF" (same UART text), not
  that a real board boots it: a real FSBL, QSPI/SD boot and the PL configuration are out of scope.
* Only one bootgen version and one set of options was available; `[load=]`, `[offset=]`,
  `[alignment=]`, `[destination_cpu=]`, split images and flash-bootable images with extra header
  padding are untested (`VERIFY:`).
* A ZynqMP image shares the two magic words; it is not told apart and would be misread.
* Follow-ups: a ps7_init replay (apply the register-init pairs and the FSBL's `ps7_init` to QEMU's
  SLCR model), FSBL execution, the second core started from a second application image.
