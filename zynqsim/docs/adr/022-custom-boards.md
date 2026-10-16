# ADR-022: Custom and additional boards

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

Prompt §10 says "custom boards are a TOML + SVG pair (Zybo Z7-10, ZedBoard, PYNQ-Z2 as
examples)" and §11 describes the visual board model. Until now `[board] name` only accepted
`zybo-z7-20`, and four places assumed four switches / four LEDs: the demo engine, the
`tel_ctrl`/`tel_event` masks (`& 0xF`) in the DPI library, the RTL adapter and the vendor GPIO
model, plus help texts. The board crate already drew everything from the TOML layout, so the work
was making the *selection* general, validating files, and letting the component counts flow
through every layer.

## Decision

### Selecting a board

```toml
[board]
name = "zedboard"        # a built-in: zybo-z7-20, zybo-z7-10, zedboard, pynq-z2
# or
file = "my-board.toml"   # relative to project.toml; `name` is then optional
```

* `BoardSection { name, file, ddr_mb }`; `name` is skipped when empty so a file-based project
  round-trips without inventing one. If both are given, `name` must equal the file's `name`
  (`board.name`); a file that does not load is `board.file`, with the loader's own message.
* `BoardDef::load(path)` / `from_toml_in(text, dir)` / `builtin(name)`; the three new built-ins
  are `boards/*.toml`, embedded with `include_str!` like the Zybo Z7-20.
* `Project::board_def(base)` resolves a project's board; `Project::validate` reports the same
  errors with their field. The resolved definition is carried, not re-looked-up by name:
  `QemuSpec.board_def`, and `SimConfig.board_def` for the demo engine. `SimConfig` therefore no
  longer derives `Eq` (the layout is `f32`).
* **Recordings carry a custom board.** `SimConfig.board_def` is serialised only when the board is
  not byte-identical to a built-in, so a recording made on `my-board.toml` replays on a machine
  without the file, and a recording on a built-in board is unchanged. The SVG text
  (`svg_source`) is `serde(skip)`: the picture is not part of the simulation.

### Board file format (schema_version 1)

Unchanged for the Zybo Z7-20, plus three optional additions: `svg = "art.svg"` (artwork behind
the controls in the 2D view), `[resources]` (`luts`, `flip_flops`, `bram_36k`, `dsp_slices`; shown
by `inspect`, checked against nothing yet) and the new connector kinds (`usb_host`, `vga`, `sd`,
`oled`, `header`, `fmc`, `xadc`, `jtag`, `clock`). The full reference is in the README.

Validation lists **every** problem at once (`BoardError::Invalid`), with precise wording:
unknown TOML tables or fields and unknown LED colours (serde, with the offending name); unknown
connector `kind` (with the list of known kinds); a control whose centre is outside the layout,
or whose body overhangs it; duplicate ids; ids that are not `[A-Za-z][A-Za-z0-9_]*`; names
derived from an id (`led5_g`, `JA1`) that collide with another id; one package pin on two parts;
bad or duplicate keyboard shortcuts; bad board names; more than **8** switches, buttons or LEDs,
more than 2 RGB LEDs, more than 6 Pmods (see the interface below).

### SVG artwork

A deliberately small subset, drawn by the existing tiny-skia renderer so the GUI, the MCP
screenshot and the golden tests share pixels: `rect` (with `rx`), `circle`, `ellipse`, `line`,
`polyline`, `polygon`, `path` with `M L H V C Q Z` (absolute and relative), `g`, and the
properties `fill`, `stroke`, `stroke-width`, `opacity`, `fill-opacity`, `stroke-opacity`
(attribute or `style`), inherited through groups. The view box is stretched over the layout
rectangle. Text, images, gradients, clipping, filters, `use` and other elements are skipped
without comment; what would draw the *wrong* picture is an error naming it (`transform`, `url(..)`
paints, percentage lengths, other path commands, unknown colour names). The file is parsed when
the board loads, so a bad SVG fails the project, not the first frame. No SVG dependency was added
(a general renderer such as `resvg` is a large tree for a schematic background). The 3D view does
not use the artwork.

### The testbench interface (the contract with RTL)

`tel_ctrl(sw, btn)` returns the slide switches and push buttons, and `tel_event({"k":"led",
"v":N})` reports the user LEDs, each as **one integer with bit *n* = component *n* of the board
file's list** (`[[switches]]` order, etc.). That is what the Zybo testbenches always did with four
bits; **the Zybo encoding is unchanged** (bits 3..0), and a board with fewer components just
leaves the upper bits zero. Boards with more components use more bits, **up to eight per group**
(the remote-port interrupt `val` is one byte): the masks `& 0xF` in `xsim-dpi`, `backend-rtl-xsim`
and `vendor-ip` became full-byte. The ZedBoard (8 switches, 7 buttons, 8 LEDs) fits exactly; a
board file asking for more is refused at validation with the reason, rather than silently losing
bits. Widening to 32 bits would need a new packet field and is not planned.

Other groups keep their encoding: Pmod ports are pin groups `0..5` in the board file's `[[pmods]]`
order (so group *n* is the *n*-th Pmod whatever it is called; `[devices] pmod_ja` = the first,
`pmod_jb` = the second...; the camera connector is group 5 and needs a `pcam` connector), and the
RGB LEDs are group 6 (bits 0..2 first RGB LED R,G,B, bits 3..5 the second).

A wider example: `fixtures/rtl/board8/` is the "virtual Zybo" GPIO block
and testbench top widened to 8 bits (`sw`, `btn`, `led`), driven by `firmware/board8_demo`
(`examples/zedboard-demo`).

### What flows from the definition

`BoardState` vectors, the 2D render, the 3D scene (already generic: extruded from the layout; a
board without art or unknown parts is a plain PCB with boxes), hit-testing, GUI keyboard shortcuts
and the status-bar hint, the GUI **board picker** in the project window (`ProjectDraft.board_name`
/ `board_file`), CLI `run --json` (`switches`, `buttons`, `leds`, `rgb`, `board_info`), `run
--board`, `inspect --board`, MCP `board_set_switch` / `board_press_button` index validation (range
errors quote the board's count), `pin_get` names and hints, `status_get.board_info`, the
`zynqsim://board` resource, and the demo engine (every LED but the last follows the switch of the
same index; the last LED is the heartbeat, as LED3 was on the Zybo).

### The new built-in boards

Zybo Z7-10, ZedBoard and PYNQ-Z2 were written **from memory** of the vendors' documents: every
pin, MIO number, LED/switch/button count, clock frequency, DDR size and peripheral list carries a
`VERIFY:` in its file and is `implemented-untested` in STATUS.md (`pin_status = "unverified"`).
Only structure is tested: that the files load, size their state, render, hit-test, extrude, map
inputs through every front end and drive a QEMU + XSim run. The ZedBoard's two extra "push
buttons" are modelled as MIO buttons (`MIO50`, `MIO51`) that the simulator nevertheless forwards to
the PL testbench; a PS-side GPIO/MIO model does not exist.

## Consequences

* Existing projects, recordings and the Zybo's behaviour are unchanged: `zybo-z7-20.toml` gained
  only a `[resources]` table; its golden PNGs are byte-identical.
* A custom board changes what is drawn and what indices mean, not the PS or PL models: Pmod
  devices, the media models and `init --xsa/--ip/--boot` still target the Zybo's connectors, and
  `init` writes `zybo-z7-20`.
* `SimConfig` gained a field and lost `Eq`; `QemuSpec` gained a field. Both are constructed in
  one place each.

## Not done

* PS-side board parts (MIO buttons and LEDs, the ZedBoard's PS Pmod JE and its OLED on PL pins)
  have no model; they are drawn or counted only.
* More than eight bits per group; Pmod/camera groups beyond the Zybo's numbering.
* SVG text, gradients, transforms, and SVG in the 3D view.
* No real board was available to confirm any pin or count of the new boards.
