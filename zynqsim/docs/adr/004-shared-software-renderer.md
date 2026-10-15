# ADR-004: One software renderer for the GUI, screenshots and golden tests

Status: **Accepted**. Date: 2026-10-15.

## Context

The board must appear in the GUI (egui), as a PNG from MCP/CLI, and in headless golden-image
tests (prompt §11, §14.7). If the GUI drew with egui's painter and the tests used something
else, a golden match would say nothing about what the user sees.

## Decision

`board-zybo::render` rasterises the board with `tiny-skia` into an RGBA buffer. The GUI uploads
that buffer as a texture each time the board state or size changes; `board_screenshot`,
`zynqsim run --screenshot` and the golden tests call the same function.

Hit-testing uses the same geometry constants (`board_zybo::geom`), so what is drawn is what is
clickable.

## Consequences

* A golden match means the GUI shows those pixels (tolerance +-2 per channel for anti-aliasing
  differences across CPUs).
* The renderer has no font. The GUI overlays labels with egui text; screenshots from MCP/CLI
  therefore have no labels. Adding a bitmap font to the renderer would close that gap.
* Rendering happens on the caller's thread from a state copy, so it never blocks the
  simulation thread.
* The prompt's `boards/*.svg` is not used yet; the board is drawn from layout data in the TOML.
  This is listed as a deviation in STATUS.md.
