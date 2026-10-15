# ADR-006: Audio and video travel over the remote-port link as bus packets

Status: **Accepted**. Date: 2026-10-15.

## Context

The board has an SSM2603 audio codec and HDMI TX/RX connectors. A design in the PL talks to them
at pin level (I2S, I2C, pixel clock/DE/HS/VS/RGB). The host side wants **samples** and **picture
lines**, not toggling pins, and it wants them in the same deterministic stream as everything else.

The RTL simulator (XSim) already has one channel to the host: the remote-port socket owned by the
`xsim-dpi` static library.

## Decision

* Pin-level models of the codec (`zs_ssm2603`: I2S DAC receiver, I2S ADC transmitter, write-only
  I2C slave at 0x1A) and the connectors (`zs_hdmi_tx_sink`, `zs_hdmi_rx_src`) are SystemVerilog
  modules in `fixtures/rtl/media/zs_media.sv`. They call `zs_*` DPI functions.
* The DPI library turns those calls into remote-port **bus packets** on reserved device numbers
  (`pl-api::media`): 14 video lines out (posted writes), 15 audio blocks out (64 stereo frames,
  posted writes), 16 video line in (reads), 17 audio block in (reads), 18 codec register writes
  (interrupt packets), plus a frame-end interrupt on 14.
* The host adapter (`backend-rtl-xsim`) answers the reads from a `VideoSource` / `AudioSource` and
  turns the writes into `MediaEvent`s that the engine (`sim-api::QemuEngine`) assembles into frames
  and a 10 s audio window. Everything is stamped with simulated time.
* A design is offered `Capability::{Video, Audio}` only when its sources mention the media models
  (`has_media`); other designs keep refusing the tools with a precise message.

## Consequences

* No extra socket, no extra thread, no change to the lock-step: media reads are answered while the
  host waits for a sync, exactly like DMA reads of PS memory.
* Audio is carried in 64-frame blocks, so output is delayed by up to 1.3 ms of simulated time and
  the last partial block is lost when the run ends. Capture is exact otherwise: frame indices are
  consecutive (tested).
* The pin models are *models*: the codec ignores the ADC gain/mute registers, accepts only 16-bit
  I2S and has no I2C read path; the HDMI models carry no EDID/DDC, no HPD and no HDMI audio.
  They are listed under "Known limits" in STATUS.md.
* The device numbers are a convention of this project, not part of the remote-port standard; a
  design that uses them needs the DPI library from this repository.
* Real-time listening is a front-end concern (the GUI queues frames for the sound card). The
  simulation is not paced by the sound card.
