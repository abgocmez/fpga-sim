# ADR-012: SD card and QSPI flash are host image files behind QEMU's models

Status: **Accepted**. Date: 2026-10-15.

## Context

The Zybo Z7-20 has a microSD slot on the PS SDIO0 controller and a 16 MiB (S25FL128S) QSPI flash
on the PS QSPI controller. Firmware that boots from or reads these needs both to behave like real
parts, and the test suite needs the same contents every run.

## Decision

* **Contents are host files.** `[storage]` in `project.toml` names a raw SD image (`sd_image`) or a
  folder that is packed into a FAT card at load (`sd_dir`), and a raw flash image (`qspi_image`) or
  a file packed at an offset (`qspi_data`, `qspi_offset`). The engine attaches them to QEMU's SDHCI
  card model and QSPI flash model; nothing is emulated in Rust.
* **Image tools are host-side Rust** in `sim-api::storage`. FAT is written by the `fatfs` crate; the
  MBR, volume id and timestamps are constants, so images are byte-for-byte reproducible. Card sizes
  are powers of two from 8 MiB to 1 GiB (QEMU's SD model refuses other sizes).
* **Read-only by default.** A guest write is not written back unless `writable = true`. Read-only
  storage keeps snapshots, reset and replay: the card and flash are the same on every re-execution.
* **Writable storage makes the session non-repeatable.** Guest writes change the host file, so the
  engine refuses snapshot save/restore, reset and recording with a message that names
  `writable = true`. Running and memory reads still work.
* **Firmware drivers are written from the TRM.** `firmware/sd_demo` has a polled SDHCI driver
  (one 512-byte block per command, PIO, 1-bit) under `embedded-sdmmc` for FAT. `firmware/qspi_demo`
  drives the QSPI controller's TX/RX FIFOs (RDID, READ) and reads the same bytes in linear mode.

## What is verified, and by what

| Claim | Test |
|---|---|
| Images round-trip files and directories; are reproducible; FAT16/FAT32 choice and small-card reason; size, label and missing-folder rules; QSPI packing at an offset; image checks name the failed rule | `sim-api` `storage::tests::*` (host only) |
| Image tools build cards and flash images and refuse bad sizes; a project with a missing or wrong image fails with a clear error | `zynqsim` `cli/tests/storage.rs` (host only, plus QEMU for the run cases) |
| The SD demo, under QEMU, reads the card's files through SDHCI and FAT (`4 file(s) read`, `RESULT: ALL PASS`), leaves a read-only image byte-identical, and repeats the same trace hash | `zynqsim` `sd_demo_reads_a_file_from_the_card_through_sdhci_and_fat` (needs Xilinx QEMU) |
| A writable card changes the image file; the demo's `sd-demo` example builds its card from its folder | `writable_storage_changes_the_image_and_refuses_what_cannot_be_repeated`, `the_sd_demo_example_builds_its_card_from_a_folder` |
| The QSPI demo reads the JEDEC ID, and the same data in FIFO and linear mode; a writable flash is programmed in the image file; each flash model reports its own ID | `qspi_demo_reads_the_jedec_id_and_flash_data_in_fifo_and_linear_mode`, `writable_flash_is_programmed_in_the_image_file`, `the_qspi_demo_example_and_each_flash_model_report_their_own_id` |
| Read-only storage: status reports the attached parts; snapshot, restore and replay reproduce the same trace hash; writable storage refuses snapshot, reset and recording | `sim-api` `tests/storage_session.rs` (two tests, Xilinx QEMU) |
| The example projects work through MCP (status, `sim_run`, UART output, snapshot round trip) | `mcp` `tests/storage.rs` (Xilinx QEMU) |

Each QEMU-dependent test prints `SKIPPED:` when Xilinx QEMU is absent; a skipped run is not a pass.

## What is not verified

* **Hardware.** The SDHCI and QSPI register offsets and bits are written from memory of the SD Host
  Controller spec and UG585; they are exercised only against QEMU's models. The firmware carries
  `VERIFY:` markers for this.
* **SD features not built:** high-speed mode, 4-bit bus, ADMA/SDMA, card-detect interrupts, SDHC
  cards above 1 GiB (the builder stops there), real card timing.
* **QSPI features not built:** dual and quad modes, DMA, interrupts, the dual-flash arrangement.
  Flash models other than the default are checked only for their JEDEC ID.
* **Other engines.** Storage is attached to the QEMU (PS) engine only. The PL engines do not see
  the card or the flash.
* **Real media.** Images are built by this project's tools; a card formatted by a PC, or a flash
  programmed by a real programmer, is not tested.
