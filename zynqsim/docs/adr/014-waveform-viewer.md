# ADR 014: Waveform viewer over the trace

Status: accepted (P2).

## Context

The engines already record a trace (`trace::TraceLog`): named `Signal` records (board switches,
buttons, LEDs, Pmod pins, camera pins, HDMI frame counter, codec registers, network counters),
UART bytes and PS/PL `Bus` accesses. It could be exported whole as VCD or Perfetto, and the GUI
showed the last 40 records as text. Debugging timing needs the opposite: pick signals, see a time
window, zoom, read values at a point.

## Decision

* **Two queries in `sim-api`**, answered by both engines from their trace log with one function
  (`sim-api::wave`, the analysis is in `trace::waveform`): `Query::SignalCatalogue` (name, width,
  number of changes, first/last time, `derived`) and `Query::Waveform { signals, from_ps, to_ps,
  max_changes }`. The viewer therefore works on the demo engine and on QEMU(+native/XSim) projects
  identically, and it reads only what is already recorded: no new instrumentation, the digest is
  untouched.
* **Signals are the VCD's signals plus three derived per bus port**: `bus.<port>.addr`,
  `.data` (32 bits, 64 when an access was longer than 4 bytes) and `.write`. They hold the value
  of the latest access, which is a faithful but coarse view of an AXI channel. They are marked
  `derived` and are not in the VCD export (the export is unchanged).
* **Bounded answers.** At most `max_changes` samples per signal (default 2000, hard cap 20000), at
  most 128 signals (a `prefix*` name expands against the catalogue). When a signal is cut, the list
  is the *first* part of the window and `covered_to_ps` says where it stops, with `truncated =
  true`; the front end shows a "truncated: zoom in" marker. The first part was chosen over
  min/max decimation because it never invents or hides an edge inside the covered range.
* **Samples are not de-duplicated**: a UART byte equal to the previous one is still an event, and
  a signal rewritten with its old value shows as a sample (as in the VCD).
* **Ordering.** Records are mostly, not strictly, time-ordered (the PL may report a pin edge after
  the PS has moved on). Samples are returned stably sorted by time, so equal times keep trace
  order, and `initial` is the last sample before the window by time.
* **One drawing for every front end.** `trace::wave_scene` lays a `Waveform` out as axis-aligned
  primitives (lines, filled rects, text) tagged with semantic tones; the GUI paints them with egui,
  MCP renders them to SVG text or to an RGBA raster (a built-in 3x5 font, encoded to PNG by the
  front end through `sim_api::board::encode_rgba_png`). Changes closer than one pixel are folded
  into an "activity" mark, so the primitive count is bounded by the width, not by the changes.
* **GUI**: a third tab "Waveforms" next to 2D/3D. Behaviour (selection, window, zoom, pan, follow,
  cursor, fetching, VCD export) is `crates/gui/src/waves.rs`, held in `Model::waves` and tested
  without a window; the egui panel is `app/wave_view.rs`. Nothing is fetched while the tab is not
  shown. A changed window is fetched at once; a live window (only new records) at most every
  100 ms. The signal list survives an engine swap (opening another project); it is not written to
  the project file, because the project file describes the simulation, not the viewer.
* **MCP**: `waveform_get` (extra tool, gated on `Capability::Trace`): no `signals` lists the
  catalogue; otherwise value changes as `[time_ps, value]` JSON, optional `render` = `svg`
  (inline or `path`) or `png` (image content, optional `path`).

## Consequences and limits

* Only boundary signals exist. PL-internal nets, IRQ lines and AXI channel handshakes are not in
  the trace and cannot be shown; for those, `[rtl] waves = true` still records an XSim `.wdb`.
* Values are `u64`; MCP JSON returns values above 2^53 as `0x...` strings.
* A catalogue or window query scans the retained trace (up to 1,000,000 records on QEMU engines),
  which costs on the order of tens of milliseconds; the GUI throttles it. There is no index.
* A bounded trace has dropped its oldest records: values before the first retained record are
  unknown (`dropped` in the answer).
* The picture is judged by eye only (colours, fonts); the tests check geometry and content.
