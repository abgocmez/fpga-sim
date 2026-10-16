//! The tool table (prompt §13).

use crate::schema::{boolean, enumeration, integer, number, object, string, string_array};
use serde_json::Value;
use sim_api::Capability;

/// What must hold for a tool to run.
#[derive(Clone, Copy, Debug)]
pub enum Gate {
    /// Always available.
    Open,
    /// Needs a backend capability the active backend set may lack.
    Needs(Capability),
    /// Not in this build; the string names the milestone that adds it.
    Planned(&'static str),
}

/// One MCP tool.
pub struct ToolDef {
    /// Tool name.
    pub name: &'static str,
    /// What it does, written for a model that has never seen the simulator.
    pub description: &'static str,
    /// Availability.
    pub gate: Gate,
    /// Argument schema.
    pub schema: Value,
}

fn t(name: &'static str, description: &'static str, gate: Gate, schema: Value) -> ToolDef {
    ToolDef {
        name,
        description,
        gate,
        schema,
    }
}

const PORT_HELP: &str = "Pmod port JA..JE, or CAM for the camera connector (the Pcam).";

const DUR: &str =
    "Simulated time such as \"250us\", \"10ms\" or \"1.5s\". Units: ps, ns, us, ms, s.";

fn duration(desc: &str) -> Value {
    string(&format!("{desc} {DUR}"), &["10ms", "250us", "1s"])
}

fn index_or_id(what: &str, id_example: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "index",
            integer(
                &format!("{what} index in board order (0-based). Give either `index` or `id`."),
                Some(0),
                Some(63),
                &[0],
            ),
        ),
        (
            "id",
            string(
                &format!("{what} id from the board file. Give either `id` or `index`."),
                &[id_example],
            ),
        ),
    ]
}

/// Every tool, in presentation order.
pub fn tool_defs() -> Vec<ToolDef> {
    use Capability as C;
    use Gate::{Needs, Open};
    let none = || object(vec![], &[]);
    vec![
        // --- project -----------------------------------------------------
        t(
            "project_open",
            "Load a project.toml (or a directory containing one), validate it and start a simulation \
             for it. The path must be inside an allowed root. Returns the diagnostics; the \
             simulation is only replaced when there are no errors.",
            Open,
            object(
                vec![(
                    "path",
                    string(
                        "Path to project.toml or its directory.",
                        &["examples/demo/project.toml"],
                    ),
                )],
                &["path"],
            ),
        ),
        t(
            "project_create_from_xsa",
            "Create a project from a Vivado hardware handoff (.xsa or .hwh): reads the PS7              configuration, every PL IP with its VLNV, address map, interrupts and clocks, writes              project.toml, a generated testbench for the user's AXI4-Lite RTL and xsa-report.md              into `out_dir`, and returns the report summary. AXI GPIO, AXI Timer and AXI UART Lite              become native vendor-ip models next to the user's RTL; IP that has no model (DMA,              IIC, ...) is listed under `unmodelled` and makes the project invalid: `runnable` is then false and project_open refuses              to start it. Existing files are never overwritten.",
            Open,
            object(
                vec![
                    (
                        "xsa_path",
                        string("Path to the .xsa (or .hwh) file.", &["design.xsa"]),
                    ),
                    (
                        "out_dir",
                        string(
                            "Directory for the project (created; project.toml must not exist).",
                            &["projects/design"],
                        ),
                    ),
                    (
                        "rtl",
                        string_array(
                            "User RTL files (Verilog/SystemVerilog) the handoff does not carry,                              for example the sources of module references.",
                            &["rtl/my_ip.v"],
                        ),
                    ),
                    (
                        "elf",
                        string(
                            "Firmware ELF for core 0 (path as it should appear in project.toml,                              relative to out_dir; default fw/app.elf).",
                            &["fw/app.elf"],
                        ),
                    ),
                ],
                &["xsa_path", "out_dir"],
            ),
        ),
        t(
            "project_validate",
            "Validate a project file (or the open project) and list every error and warning with its \
             field path, without starting a simulation.",
            Open,
            object(
                vec![(
                    "path",
                    string(
                        "Path to project.toml; defaults to the open project.",
                        &["project.toml"],
                    ),
                )],
                &[],
            ),
        ),
        // --- run control -------------------------------------------------
        t(
            "sim_run",
            "Advance simulated time by a duration, firing every event on the way. Long runs are \
             chunked, report progress and honour cancellation. Maximum 3600 s per call.",
            Open,
            object(
                vec![("duration", duration("How far to advance."))],
                &["duration"],
            ),
        ),
        t(
            "sim_pause",
            "Stop continuous running. Has no effect when not running; simulated time never advances \
             between tool calls unless a run was started elsewhere (for example by the GUI).",
            Open,
            none(),
        ),
        t(
            "sim_step",
            "Advance by exactly one lock-step quantum (see status_get for its length).",
            Open,
            none(),
        ),
        t(
            "sim_run_until",
            "Advance simulated time to an absolute time. A no-op if that time has already passed.",
            Open,
            object(vec![("time", duration("Absolute target time."))], &["time"]),
        ),
        t(
            "sim_reset",
            "Reset the system at the current time, optionally re-seeding the PRNG. The reset is \
             recorded so replays reproduce it.",
            Open,
            object(
                vec![(
                    "seed",
                    integer(
                        "New PRNG seed; keeps the current seed when omitted.",
                        Some(0),
                        None,
                        &[42],
                    ),
                )],
                &[],
            ),
        ),
        // --- state -------------------------------------------------------
        t(
            "snapshot_save",
            "Save the complete simulation state under a name.",
            Needs(C::Snapshot),
            object(
                vec![("name", string("Snapshot name.", &["before-button"]))],
                &["name"],
            ),
        ),
        t(
            "snapshot_restore",
            "Rewind to a saved snapshot, including simulated time, trace and recording.",
            Needs(C::Snapshot),
            object(
                vec![("name", string("Snapshot name.", &["before-button"]))],
                &["name"],
            ),
        ),
        t(
            "session_replay",
            "Replay the recorded inputs (or a recording file inside an allowed root) on a fresh \
             simulator and report whether the trace digest matches. This is the determinism check.",
            Needs(C::Replay),
            object(
                vec![(
                    "path",
                    string(
                        "Recording JSON file; defaults to the current session.",
                        &["session.json"],
                    ),
                )],
                &[],
            ),
        ),
        // --- PS inspection (need a PS backend) -----------------------------
        t(
            "cpu_regs",
            "Read r0-r15, CPSR and the mode of a Cortex-A9 core. Only possible while the guest is \
             stopped at a breakpoint or watchpoint (the project must have `[debug] enabled = \
             true`): set one with `breakpoint_set`, `sim_run`, and read the registers when the \
             result says `debug.stopped`. A paused-but-not-stopped machine cannot be read. When \
             the ELF has DWARF line info the result has `pc_source` (file and line) and \
             `source_context` (the lines around it, when the file is found).",
            Needs(C::CpuState),
            object(
                vec![("core", integer("Core index.", Some(0), Some(1), &[0]))],
                &[],
            ),
        ),
        t(
            "cpu_disasm",
            "Disassemble ARM or Thumb instructions from guest memory at an address, annotated \
             with symbols of core 0's ELF. Works while the machine is paused. `thumb` defaults \
             to the CPSR T bit when core 0 is stopped under the debugger, else ARM.",
            Needs(C::MemoryAccess),
            object(
                vec![
                    (
                        "addr",
                        integer("Start address.", Some(0), None, &[0x100000]),
                    ),
                    (
                        "count",
                        integer("Number of instructions.", Some(1), Some(256), &[16]),
                    ),
                    ("thumb", boolean("Decode as Thumb instead of ARM.")),
                ],
                &["addr"],
            ),
        ),
        t(
            "mem_read",
            "Read physical memory.",
            Needs(C::MemoryAccess),
            object(
                vec![
                    (
                        "addr",
                        integer("Physical address.", Some(0), None, &[0x43C00000]),
                    ),
                    (
                        "len",
                        integer("Bytes to read (max 4096).", Some(1), Some(4096), &[16]),
                    ),
                ],
                &["addr", "len"],
            ),
        ),
        t(
            "mem_write",
            "Write physical memory.",
            Needs(C::MemoryAccess),
            object(
                vec![
                    (
                        "addr",
                        integer("Physical address.", Some(0), None, &[0x43C00000]),
                    ),
                    ("hex", string("Bytes as a hex string.", &["deadbeef"])),
                ],
                &["addr", "hex"],
            ),
        ),
        t(
            "symbol_lookup",
            "Resolve an address to an ELF symbol or a symbol to an address (core 0's ELF).",
            Needs(C::MemoryAccess),
            object(
                vec![(
                    "query",
                    string("Address (0x...) or symbol name.", &["main"]),
                )],
                &["query"],
            ),
        ),
        t(
            "source_line",
            "The source position of a code address, from the ELF's DWARF line table: file, \
             line, column, function, and a few numbered lines of the file around it (`lines`, \
             the current one marked). Give `addr`, a `symbol` or a `location` (`main.rs:42`). \
             The file is found through the compilation directory recorded in the ELF, the \
             project directory and `[debug] source_dirs`; if it is not found the position is \
             still returned with `source_unavailable`. Fails if the ELF was built without \
             debug info.",
            Needs(C::MemoryAccess),
            object(
                vec![
                    ("addr", integer("Code address.", Some(0), None, &[0x100000])),
                    ("symbol", string("ELF symbol name or address.", &["main"])),
                    (
                        "location",
                        string("Source line `file.rs:LINE`.", &["main.rs:42"]),
                    ),
                    (
                        "context",
                        integer(
                            "Lines of context on each side (default 3).",
                            Some(0),
                            Some(50),
                            &[3],
                        ),
                    ),
                ],
                &[],
            ),
        ),
        t(
            "breakpoint_set",
            "Set (or clear, with `enable` false) an execution breakpoint at an address \
             (`addr`) or at a `location`: a symbol, a hex address or a source line \
             `file.rs:LINE` (needs DWARF line info; a line that starts code in several places \
             gets one breakpoint at each). `sim_run` then stops \
             when the guest reaches it and reports `debug.stopped`; the next `sim_run` continues. \
             Needs a project with `[debug] enabled = true`. Setting one while the machine is \
             merely paused takes effect within the next quantum, at a wall-clock-dependent \
             instant; breakpoints listed in `[debug] breakpoints` are exact.",
            Needs(C::CpuState),
            object(
                vec![
                    ("addr", integer("Address.", Some(0), None, &[0x100000])),
                    (
                        "location",
                        string(
                            "Symbol, hex address or `file.rs:LINE` (instead of `addr`).",
                            &["main.rs:42"],
                        ),
                    ),
                    ("enable", boolean("False clears it (default true).")),
                ],
                &[],
            ),
        ),
        t(
            "watchpoint_set",
            "Set (or clear) a data watchpoint that stops the guest on a read or write of the \
             bytes. Needs a project with `[debug] enabled = true`.",
            Needs(C::CpuState),
            object(
                vec![
                    ("addr", integer("Address.", Some(0), None, &[0x43C00000])),
                    ("len", integer("Length in bytes.", Some(1), Some(64), &[4])),
                    (
                        "access",
                        enumeration(
                            "What triggers it (default `write`).",
                            &["write", "read", "access"],
                        ),
                    ),
                    ("enable", boolean("False clears it (default true).")),
                ],
                &["addr"],
            ),
        ),
        t(
            "breakpoint_list",
            "List breakpoints and watchpoints.",
            Needs(C::CpuState),
            none(),
        ),
        // --- console -------------------------------------------------------
        t(
            "uart_write",
            "Send bytes to the target on a UART (host to target). Give `text` or `hex`. The bytes \
             are delivered at the current simulated time; run the simulation to see the reaction.",
            Needs(C::Uart),
            object(
                vec![
                    (
                        "port",
                        integer(
                            "UART index: 0 or 1 are the PS UARTs, 2.. the PL's AXI UART Lite instances in name order.",
                            Some(0),
                            Some(7),
                            &[1],
                        ),
                    ),
                    ("text", string("Text to send (UTF-8).", &["hello\r\n"])),
                    ("hex", string("Bytes to send as hex.", &["68690d0a"])),
                ],
                &["port"],
            ),
        ),
        t(
            "uart_read",
            "Read bytes the target has transmitted on a UART. Without `from`, returns only bytes not \
             yet read by this server (a cursor per UART); with `from`, reads from that absolute \
             offset without moving the cursor.",
            Needs(C::Uart),
            object(
                vec![
                    (
                        "port",
                        integer(
                            "UART index: 0 or 1 are the PS UARTs, 2.. the PL's AXI UART Lite instances in name order.",
                            Some(0),
                            Some(7),
                            &[1],
                        ),
                    ),
                    (
                        "from",
                        integer("Absolute stream offset.", Some(0), None, &[0]),
                    ),
                    (
                        "max",
                        integer(
                            "Maximum bytes (default 4096, max 65536).",
                            Some(1),
                            Some(65536),
                            &[256],
                        ),
                    ),
                ],
                &["port"],
            ),
        ),
        // --- board ---------------------------------------------------------
        t(
            "board_set_switch",
            "Move a slide switch on the board to the on or off position, at the current simulated \
             time. Identify it by `index` or `id`. Run the simulation afterwards to see the effect \
             propagate.",
            Needs(C::BoardIo),
            object(
                {
                    let mut p = index_or_id("Switch", "sw0");
                    p.push(("on", boolean("True for the on position.")));
                    p
                },
                &["on"],
            ),
        ),
        t(
            "board_press_button",
            "Press and/or release a push button. `tap` (default) presses, runs the simulation for \
             `hold`, then releases.",
            Needs(C::BoardIo),
            object(
                {
                    let mut p = index_or_id("Button", "btn0");
                    p.push((
                        "action",
                        enumeration("tap, down or up.", &["tap", "down", "up"]),
                    ));
                    p.push(("hold", duration("How long a tap is held (default 50ms).")));
                    p
                },
                &[],
            ),
        ),
        t(
            "board_get_leds",
            "Read every LED: user LEDs as duty 0-255 and RGB LEDs as r/g/b duty.",
            Needs(C::BoardIo),
            none(),
        ),
        t(
            "pmod_attach",
            "Attach a Pmod module to a port JA..JE so its display and state follow the pins the \
             PL design drives. Available: `oled` (128x32 SSD1306 SPI, Digilent Pmod OLED pinout: \
             1 CS, 2 SDIN, 4 SCLK, 7 D/C, 8 RES); `tmp2` (Pmod TMP2, ADT7420 temperature sensor, \
             I2C 0x4B: 3 SCL, 4 SDA); `als` (Pmod ALS, ADC081S021 light sensor, SPI mode 0: \
             1 CS, 3 SDO, 4 SCLK); `acl` (Pmod ACL, ADXL345 accelerometer, SPI mode 3: 1 CS, \
             2 SDI, 3 SDO, 4 SCLK); `pcam` (Pcam 5C, OV5640 image sensor: SCCB at I2C 0x3C on \
             the camera connector CAM, frames through the PL's video input; MIPI CSI-2 is not \
             simulated). Only RTL designs that report their Pmod pins (the \
             `zs_pin_out` / `zs_pin_xchg` DPI, see fixtures/rtl/pmod) have a module to talk to. \
             Modules can also be declared in project.toml (`[devices] pmod_ja = \"oled\"`). Set \
             what a sensor measures with `pmod_set`.",
            Needs(C::Pmod),
            object(
                vec![
                    (
                        "port",
                        enumeration(PORT_HELP, &["JA", "JB", "JC", "JD", "JE", "CAM"]),
                    ),
                    ("module", enumeration("Module kind.", sim_api::DEVICE_KINDS)),
                ],
                &["port", "module"],
            ),
        ),
        t(
            "pmod_frame",
            "The picture the module on a Pmod port shows, as a PNG (the OLED's 128x32 display), \
             with its state line.",
            Needs(C::Pmod),
            object(
                vec![(
                    "port",
                    enumeration(PORT_HELP, &["JA", "JB", "JC", "JD", "JE", "CAM"]),
                )],
                &["port"],
            ),
        ),
        t(
            "pmod_set",
            "Set a physical quantity a Pmod sensor measures: `temperature` in degC for `tmp2` \
             (-55..150), `light` as the 8-bit ADC code 0..255 for `als`, `accel_x`, `accel_y`, \
             `accel_z` in g for `acl` (+-16). The value is clamped to the model's range and \
             reaches the PL at the next time the firmware reads the sensor, so the result is \
             deterministic. Returns the stored value and the model state. `pmod_state` lists \
             what a module accepts.",
            Needs(C::Pmod),
            object(
                vec![
                    (
                        "port",
                        enumeration(PORT_HELP, &["JA", "JB", "JC", "JD", "JE", "CAM"]),
                    ),
                    (
                        "name",
                        string(
                            "Quantity name (see pmod_state).",
                            &["temperature", "light", "accel_z"],
                        ),
                    ),
                    (
                        "value",
                        number("New value, in the quantity's unit.", None, None, &[23.5]),
                    ),
                ],
                &["port", "name", "value"],
            ),
        ),
        t(
            "pmod_state",
            "The state of the module on a Pmod port: its settable quantities (name, value, unit, \
             range), its registers and a one-line description.",
            Needs(C::Pmod),
            object(
                vec![(
                    "port",
                    enumeration(PORT_HELP, &["JA", "JB", "JC", "JD", "JE", "CAM"]),
                )],
                &["port"],
            ),
        ),
        t(
            "pin_get",
            "Read the logic level (0, 1 or Z) of a named pin or control. Pmod pins follow what the \
             PL design drives (see `status_get` capabilities for `pmod`).",
            Open,
            object(
                vec![(
                    "name",
                    string(
                        "sw0..sw3, btn0..btn3, led0..led3, led5_r/g/b, JA1..JF10.",
                        &["led0", "JA1"],
                    ),
                )],
                &["name"],
            ),
        ),
        // --- PL observation --------------------------------------------------
        t(
            "pl_signal_get",
            "Read the current value of a PL signal by hierarchical name.",
            Needs(C::PlSignals),
            object(
                vec![(
                    "name",
                    string("Hierarchical signal name.", &["top.u_accel.state"]),
                )],
                &["name"],
            ),
        ),
        t(
            "pl_signal_trace",
            "Record value changes of PL signals over a time window.",
            Needs(C::PlSignals),
            object(
                vec![
                    (
                        "names",
                        string("Comma-separated signal names.", &["top.clk,top.irq"]),
                    ),
                    ("duration", duration("Window length.")),
                ],
                &["names", "duration"],
            ),
        ),
        t(
            "pl_axi_log",
            "List AXI transactions seen on a PL port, with latencies and protocol-checker findings.",
            Needs(C::PlSignals),
            object(
                vec![("port", string("AXI port name.", &["M_AXI_GP0"]))],
                &[],
            ),
        ),
        t(
            "waveform_export",
            "Export the trace as VCD (for GTKWave) or Perfetto JSON. With `path` (inside an allowed \
             root) the file is written and its size returned; otherwise the text is returned, \
             truncated at 200 KiB.",
            Needs(C::Trace),
            object(
                vec![
                    (
                        "format",
                        enumeration("vcd or perfetto.", &["vcd", "perfetto"]),
                    ),
                    (
                        "path",
                        string("Output file inside an allowed root.", &["out/run.vcd"]),
                    ),
                ],
                &["format"],
            ),
        ),
        t(
            "waveform_get",
            "Read the value changes of chosen signals inside a time window, for debugging timing:              `signals` are names from the catalogue (call with no `signals` to list them: board              switches/buttons/LEDs `board.*`, Pmod pins `pmod.*`, `uart1.tx`, `net.*`, and              `bus.<port>.addr|data|write` derived from every PS/PL bus access). A name ending in              `*` matches by prefix. Times are picoseconds; `changes` are `[time_ps, value]`              and `initial` is the value in effect at `from`. The answer is bounded:              `truncated` is true when a signal had more than `max_changes` changes in the window              (it is complete up to its `covered_to_ps`; narrow the window to see the rest). Only              signals the engine recorded at the PL boundary exist, not the PL's internal nets.              `render` adds an SVG (inline, or in `path`) or a PNG image of the same window.",
            Needs(C::Trace),
            object(
                vec![
                    (
                        "signals",
                        string_array(
                            "Signal names or `prefix*` patterns; omit to list the catalogue.",
                            &["board.led0", "uart1.tx", "bus.M_AXI_GP0.*"],
                        ),
                    ),
                    ("from", duration("Start of the window (default 0).")),
                    (
                        "to",
                        duration("End of the window (default: the newest record)."),
                    ),
                    (
                        "max_changes",
                        integer(
                            "Most changes per signal (default 2000, max 20000).",
                            Some(1),
                            Some(20000),
                            &[500],
                        ),
                    ),
                    (
                        "render",
                        enumeration(
                            "Also draw the window: svg, png or none (default).",
                            &["none", "svg", "png"],
                        ),
                    ),
                    (
                        "width",
                        integer(
                            "Picture width in pixels (default 1000).",
                            Some(200),
                            Some(4000),
                            &[1000],
                        ),
                    ),
                    (
                        "path",
                        string(
                            "Write the SVG/PNG here (inside an allowed root) instead of inline.",
                            &["out/wave.svg"],
                        ),
                    ),
                ],
                &[],
            ),
        ),
        t(
            "trace_get",
            "Read a page of trace records (inputs, signal changes, UART bytes, markers).",
            Needs(C::Trace),
            object(
                vec![
                    (
                        "from",
                        integer("Index of the first record.", Some(0), None, &[0]),
                    ),
                    (
                        "max",
                        integer(
                            "Maximum records (default 100, max 1000).",
                            Some(1),
                            Some(1000),
                            &[50],
                        ),
                    ),
                ],
                &[],
            ),
        ),
        // --- media / network ---------------------------------------------------
        t(
            "video_frame",
            "Return the newest complete HDMI TX frame as a PNG image. The PL must have enabled its \
             scan-out and the simulation must have run long enough to produce a frame; the result \
             says what to do otherwise.",
            Needs(C::Video),
            none(),
        ),
        t(
            "media_source_set",
            "Choose what the board receives: `video_in` is `bars[:WxH]`, `gradient[:WxH]` or the \
             path of a PNG; `audio_in` is `silence`, `tone:HZ[:AMPLITUDE]` or the path of a 16-bit \
             WAV (resampled to 48 kHz). At least one is required. Files must be inside an allowed \
             root.",
            Needs(C::Video),
            object(
                vec![
                    (
                        "video_in",
                        string(
                            "HDMI RX picture.",
                            &["bars", "gradient:640x480", "photo.png"],
                        ),
                    ),
                    (
                        "audio_in",
                        string(
                            "Codec LINE IN audio.",
                            &["silence", "tone:440:0.5", "speech.wav"],
                        ),
                    ),
                ],
                &[],
            ),
        ),
        t(
            "board_screenshot",
            "Render the board (LEDs, switches, buttons, Pmod pins) as a PNG image.",
            Needs(C::Screenshot),
            object(
                vec![(
                    "scale",
                    number(
                        "Pixels per layout unit, 0.25 to 4 (default 1).",
                        Some(0.25),
                        Some(4.0),
                        &[1.0],
                    ),
                )],
                &[],
            ),
        ),
        t(
            "audio_capture",
            "Return the most recent audio the codec DAC produced, as a WAV file, with its peak, RMS \
             level and dominant frequency. It reads audio the simulation has already produced (the \
             newest `duration`, at most 10 s are retained); run the simulation first.",
            Needs(C::Audio),
            object(
                vec![(
                    "duration",
                    duration("Length of the newest audio to return."),
                )],
                &["duration"],
            ),
        ),
        t(
            "net_send",
            "Inject an Ethernet frame towards the target. Not available with the user-mode \
             network the simulator provides: it answers `unsupported` and says why (ADR-011).",
            Needs(C::Network),
            object(
                vec![("hex", string("Frame bytes as hex.", &["ffffffffffff"]))],
                &["hex"],
            ),
        ),
        t(
            "net_capture",
            "Return the frames the board's Ethernet (GEM0) sent and received, oldest first, with \
             virtual-time stamps, a one-line summary each, and the frame counters. Frames are \
             captured by QEMU's network stack at the instant the guest transmits, so the list is \
             identical on every run of the same project. Use `from`/`next` to page.",
            Needs(C::Network),
            object(
                vec![
                    (
                        "from",
                        integer(
                            "Index of the first frame wanted, counted from the start of the \
                             run (default 0).",
                            Some(0),
                            None,
                            &[0],
                        ),
                    ),
                    (
                        "max",
                        integer(
                            "At most this many frames (default 50, at most 500).",
                            Some(1),
                            Some(500),
                            &[50],
                        ),
                    ),
                ],
                &[],
            ),
        ),
        // --- configuration -------------------------------------------------------
        t(
            "config_set",
            "Change simulator configuration. `quantum` sets the lock-step quantum. `seed` re-seeds \
             the PRNG and resets the system (recorded, so replays match). Fidelity is determined by \
             the backends and cannot be set.",
            Open,
            object(
                vec![
                    ("quantum", duration("Lock-step quantum.")),
                    (
                        "seed",
                        integer("New PRNG seed (resets the system).", Some(0), None, &[7]),
                    ),
                ],
                &[],
            ),
        ),
        t(
            "status_get",
            "Time, backends, fidelity level, capabilities, trace digest and the open project. Check \
             `fidelity` and `capabilities` before trusting a result.",
            Open,
            none(),
        ),
        t(
            "backend_list",
            "List PS and PL backends, whether each is implemented in this build, and whether the \
             external program it needs was found on PATH.",
            Open,
            none(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The tool names required by prompt §13, plus documented extensions.
    const SPEC: &[&str] = &[
        "project_open",
        "project_create_from_xsa",
        "project_validate",
        "sim_run",
        "sim_pause",
        "sim_step",
        "sim_run_until",
        "sim_reset",
        "snapshot_save",
        "snapshot_restore",
        "session_replay",
        "cpu_regs",
        "cpu_disasm",
        "mem_read",
        "mem_write",
        "symbol_lookup",
        "breakpoint_set",
        "watchpoint_set",
        "breakpoint_list",
        "uart_write",
        "uart_read",
        "board_set_switch",
        "board_press_button",
        "board_get_leds",
        "pmod_attach",
        "pin_get",
        "pl_signal_get",
        "pl_signal_trace",
        "pl_axi_log",
        "waveform_export",
        "video_frame",
        "board_screenshot",
        "audio_capture",
        "net_send",
        "net_capture",
        "config_set",
        "status_get",
        "backend_list",
    ];

    #[test]
    fn covers_every_tool_in_the_spec_exactly_once() {
        let names: Vec<&str> = tool_defs().iter().map(|d| d.name).collect();
        let set: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(set.len(), names.len(), "duplicate tool names");
        for s in SPEC {
            assert!(set.contains(s), "missing spec tool {s}");
        }
        let mut extra: Vec<_> = names.iter().filter(|n| !SPEC.contains(n)).collect();
        extra.sort();
        assert_eq!(
            extra,
            vec![
                &"media_source_set",
                &"pmod_frame",
                &"pmod_set",
                &"pmod_state",
                &"source_line",
                &"trace_get",
                &"waveform_get"
            ],
            "undocumented extensions"
        );
    }

    #[test]
    fn every_tool_and_parameter_is_documented() {
        for d in tool_defs() {
            assert!(
                d.description.len() > 20,
                "{} needs a real description",
                d.name
            );
            assert_eq!(d.schema["type"], "object");
            assert_eq!(d.schema["additionalProperties"], false);
            for (k, p) in d.schema["properties"].as_object().unwrap() {
                assert!(
                    p["description"].as_str().is_some_and(|s| s.len() > 5),
                    "{}.{k} lacks a description",
                    d.name
                );
                assert!(p.get("type").is_some(), "{}.{k} lacks a type", d.name);
            }
        }
    }
}
