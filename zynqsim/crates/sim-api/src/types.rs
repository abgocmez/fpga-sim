use board_zybo::{BoardDef, BoardState, PinLevel};
use serde::{Deserialize, Serialize};
use trace::TraceRecord;

/// How faithful the active backends are (prompt §5). Reported with every
/// status so no result is ever read as more exact than it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// A built-in scripted model with no CPU or RTL behind it. Exercises the
    /// control plane, board and trace only.
    Demo,
    /// Functional behaviour with instruction-count timing.
    F0,
    /// Annotated timing (native PS backend only).
    F1,
    /// Detailed timing (native PS backend only).
    F2,
}

/// Something a backend set can do. Front ends use this to say *why* a request
/// cannot be served instead of failing silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Switches, buttons, LEDs and pins.
    BoardIo,
    /// UART console.
    Uart,
    /// Event trace, VCD and Perfetto export.
    Trace,
    /// Save and restore of the full simulation state.
    Snapshot,
    /// Recording and deterministic replay of front-end inputs.
    Replay,
    /// Headless board rendering.
    Screenshot,
    /// CPU registers, disassembly, breakpoints.
    CpuState,
    /// Physical-memory access.
    MemoryAccess,
    /// PL signal inspection and AXI monitors.
    PlSignals,
    /// HDMI video frames.
    Video,
    /// Audio capture.
    Audio,
    /// Network capture (the board's Ethernet on a simulated LAN). Frame
    /// injection is not offered by the user-mode network backend (ADR-011).
    Network,
    /// Pmod ports and the RGB LEDs driven by the PL design, and the devices
    /// attached to them.
    Pmod,
    /// An SD card image and/or QSPI flash image is attached to the board.
    Storage,
}

/// An input applied to the simulated system at the current simulation time.
/// Inputs are the only external stimuli, so recording them is enough to replay
/// a session exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    /// Move a slide switch.
    SetSwitch {
        /// Switch index in board order.
        index: usize,
        /// True for the on position.
        on: bool,
    },
    /// Press or release a push button.
    SetButton {
        /// Button index in board order.
        index: usize,
        /// True while pressed.
        pressed: bool,
    },
    /// Send bytes to the target over a UART (host to target).
    UartWrite {
        /// UART index, 0 or 1.
        port: u8,
        /// Bytes to send.
        data: Vec<u8>,
    },
    /// Reset the system, optionally re-seeding the PRNG.
    Reset {
        /// New seed; the current seed is kept when absent.
        seed: Option<u64>,
    },
}

/// Where the HDMI RX connector gets its picture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VideoSpec {
    /// Eight colour bars over a grey ramp.
    ColorBars {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// A red/green/blue gradient.
    Gradient {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// A still image.
    Image {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `3 * width * height` bytes, R, G, B.
        rgb: Vec<u8>,
    },
}

/// Where the codec's LINE IN gets its audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSpec {
    /// Digital silence.
    Silence,
    /// A sine tone.
    Tone {
        /// Frequency in hertz.
        freq_hz: f64,
        /// Peak amplitude, 0.0 to 1.0.
        amplitude: f64,
    },
    /// Interleaved stereo 48 kHz PCM, looped.
    Pcm {
        /// The samples.
        samples: Vec<i16>,
    },
}

/// What a breakpoint or watchpoint reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakKind {
    /// An instruction at the address is about to execute.
    Exec,
    /// The guest writes the watched bytes.
    Write,
    /// The guest reads the watched bytes.
    Read,
    /// The guest reads or writes the watched bytes.
    Access,
}

/// A breakpoint or watchpoint that is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointInfo {
    /// Address.
    pub addr: u64,
    /// Kind.
    pub kind: BreakKind,
    /// Length in bytes (watchpoints).
    pub len: u32,
    /// ELF symbol at the address, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Registers of one core, read while it is stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuRegs {
    /// Core index.
    pub core: u8,
    /// r0..r15 (`r[13]` sp, `r[14]` lr, `r[15]` pc).
    pub r: [u32; 16],
    /// Current program status register.
    pub cpsr: u32,
    /// Processor mode decoded from the CPSR (`svc`, `sys`, `irq`...).
    pub mode: String,
    /// ELF symbol and offset of the pc, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc_symbol: Option<String>,
}

/// An ELF symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolInfo {
    /// Symbol name.
    pub name: String,
    /// Start address.
    pub addr: u64,
    /// Size in bytes (0 when the ELF does not say).
    pub size: u64,
    /// Offset of the queried address into the symbol (0 for a name query).
    pub offset: u64,
}

/// One disassembled instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisasmLine {
    /// Address.
    pub addr: u64,
    /// Encoded size in bytes.
    pub size: u32,
    /// Raw bytes, hex, in memory order.
    pub hex: String,
    /// Assembly text.
    pub text: String,
    /// `symbol` or `symbol+0xoff` at this address, when the ELF knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Debugger state of an engine started with a debugger attached.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugStatus {
    /// True while the guest is stopped at a breakpoint, watchpoint or step.
    pub stopped: bool,
    /// The core that stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<u8>,
    /// Its program counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<u32>,
    /// Why it stopped, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Stops so far.
    pub stops: u32,
}

/// A device attached to a Pmod port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    /// Pmod port index: 0..=4 are JA..JE.
    pub port: u8,
    /// Device kind (`oled`).
    pub kind: String,
    /// One line of the device's state.
    pub description: String,
}

/// A physical quantity of a device model the host can set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceValueInfo {
    /// Name (`temperature`).
    pub name: String,
    /// Current value.
    pub value: f64,
    /// Unit (`degC`).
    pub unit: String,
    /// Smallest value the model represents.
    pub min: f64,
    /// Largest value the model represents.
    pub max: f64,
}

/// A register of a device model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRegisterInfo {
    /// Register name.
    pub name: String,
    /// Address in the device's register map.
    pub addr: u32,
    /// Content.
    pub value: u32,
}

/// A request that changes the simulation.
///
/// Adjacently tagged (`{"type": .., "args": ..}`) because `Input` is itself
/// internally tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "args", rename_all = "snake_case")]
pub enum Command {
    /// Apply a recorded input.
    Input(Input),
    /// Advance simulated time by a duration.
    Run {
        /// Duration in picoseconds.
        duration_ps: u64,
    },
    /// Advance simulated time to an absolute time.
    RunUntil {
        /// Target time in picoseconds; a no-op if already past.
        time_ps: u64,
    },
    /// Advance by exactly one quantum.
    Step,
    /// Run continuously at a fixed simulated-to-wall-clock ratio until paused.
    Play {
        /// Simulated picoseconds per wall-clock second.
        ps_per_second: u64,
    },
    /// Stop continuous running.
    Pause,
    /// Attach a device model to a Pmod port. Needs [`Capability::Pmod`].
    AttachDevice {
        /// Pmod port index: 0..=4 are JA..JE.
        port: u8,
        /// Device kind (`oled`).
        kind: String,
    },
    /// Set a physical quantity of the device on a Pmod port (the
    /// temperature a sensor measures). Needs [`Capability::Pmod`].
    SetDeviceValue {
        /// Pmod port index: 0..=4 are JA..JE.
        port: u8,
        /// Quantity name (`temperature`, see `Query::DeviceState`).
        name: String,
        /// New value, clamped to the model's range.
        value: f64,
    },
    /// Set or clear a breakpoint or watchpoint. Needs [`Capability::CpuState`]
    /// (a project started with `[debug]`).
    SetBreakpoint {
        /// Address.
        addr: u64,
        /// Kind.
        kind: BreakKind,
        /// Watched length in bytes (ignored for execution breakpoints).
        len: u32,
        /// False removes it.
        enable: bool,
    },
    /// Choose the HDMI RX picture and/or the codec LINE IN audio.
    /// Needs [`Capability::Video`] or [`Capability::Audio`].
    SetMediaSource {
        /// New video source, if changing.
        video: Option<VideoSpec>,
        /// New audio source, if changing.
        audio: Option<AudioSpec>,
    },
    /// Write guest physical memory. Needs [`Capability::MemoryAccess`].
    MemWrite {
        /// Physical address.
        addr: u64,
        /// Bytes to write.
        data: Vec<u8>,
    },
    /// Change the lock-step quantum used by `Step` and continuous running.
    SetQuantum {
        /// New quantum in picoseconds; must be non-zero.
        quantum_ps: u64,
    },
    /// Save the full state under a name.
    SnapshotSave {
        /// Snapshot name.
        name: String,
    },
    /// Restore a previously saved state.
    SnapshotRestore {
        /// Snapshot name.
        name: String,
    },
}

/// Output format for waveform/trace export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceFormat {
    /// Value Change Dump.
    Vcd,
    /// Chrome trace-event JSON, loadable in Perfetto.
    Perfetto,
}

/// A read-only request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Query {
    /// Time, backend, fidelity and capabilities.
    Status,
    /// The board description.
    BoardDef,
    /// Switches, buttons, LEDs and pins.
    BoardState,
    /// The level of one named pin or control.
    Pin {
        /// Pin or control name (`sw0`, `led2`, `led5_g`, `JA1`).
        name: String,
    },
    /// Target-to-host UART bytes from an absolute stream offset.
    UartRead {
        /// UART index.
        port: u8,
        /// Absolute byte offset to read from.
        from: u64,
        /// Maximum bytes to return.
        max: usize,
    },
    /// A page of the trace.
    Trace {
        /// Index of the first record.
        from: usize,
        /// Maximum number of records.
        max: usize,
    },
    /// The whole trace in an export format.
    TraceExport {
        /// Format.
        format: TraceFormat,
    },
    /// The signals of the trace (name, width, activity), for a waveform
    /// viewer's signal picker. Needs [`Capability::Trace`].
    SignalCatalogue,
    /// The value changes of named signals inside a time window. A name
    /// ending in `*` matches by prefix. The answer is bounded and says when
    /// it was cut. Needs [`Capability::Trace`].
    Waveform {
        /// Signal names, as listed by [`Query::SignalCatalogue`].
        signals: Vec<String>,
        /// Start of the window in picoseconds.
        from_ps: u64,
        /// End of the window in picoseconds (inclusive).
        to_ps: u64,
        /// Most value changes per signal; clamped to 1..=20000.
        max_changes: usize,
    },
    /// The input recording so far.
    Recording,
    /// Names of saved snapshots.
    Snapshots,
    /// The board rendered as PNG.
    Screenshot {
        /// Pixels per layout unit (0.25..=4).
        scale: f32,
    },
    /// The most recent complete HDMI TX frame as PNG. Needs [`Capability::Video`].
    VideoFrame,
    /// The most recent codec DAC audio as a WAV. Needs [`Capability::Audio`].
    AudioCapture {
        /// At most this many stereo frames (the newest); 0 means all retained.
        max_frames: u32,
    },
    /// Codec DAC audio from a given frame on, for streaming to a speaker.
    /// Frames already dropped from the 10 s window are skipped; an empty answer
    /// means nothing new. Needs [`Capability::Audio`].
    AudioFrom {
        /// First wanted stereo frame, counted from the start of the run.
        from_frame: u64,
        /// At most this many frames.
        max_frames: u32,
    },
    /// CPU registers of a core. Needs [`Capability::CpuState`].
    CpuRegs {
        /// Core index.
        core: u8,
    },
    /// The picture of the device on a Pmod port. Needs [`Capability::Pmod`].
    DeviceImage {
        /// Pmod port index.
        port: u8,
    },
    /// The state of the device on a Pmod port: settable quantities and
    /// registers. Needs [`Capability::Pmod`].
    DeviceState {
        /// Pmod port index.
        port: u8,
    },
    /// Disassemble guest memory. Needs [`Capability::MemoryAccess`].
    Disasm {
        /// Start address.
        addr: u64,
        /// Instructions wanted.
        count: u32,
        /// Thumb (true) or ARM (false); by default the CPSR T bit of core 0
        /// when it is stopped, else ARM.
        thumb: Option<bool>,
    },
    /// Breakpoints and watchpoints. Needs [`Capability::CpuState`].
    Breakpoints,
    /// Resolve an address (`0x...`) or a name against the core 0 ELF. Needs
    /// [`Capability::CpuState`].
    Symbol {
        /// Address or symbol name.
        query: String,
    },
    /// Physical memory. Needs [`Capability::MemoryAccess`].
    MemRead {
        /// Physical address.
        addr: u64,
        /// Number of bytes.
        len: u32,
    },
    /// A PL signal. Needs [`Capability::PlSignals`].
    PlSignal {
        /// Hierarchical signal name.
        name: String,
    },
    /// Frames of the board's Ethernet, oldest first, from an absolute index.
    /// Needs [`Capability::Network`].
    NetCapture {
        /// Index of the first wanted frame, counted from the start of the run.
        from: u64,
        /// At most this many frames.
        max: usize,
    },
    /// Frame counters of the board's Ethernet. Needs [`Capability::Network`].
    NetStats,
    /// Every retained frame as a classic `.pcap` file with virtual-time stamps.
    /// Needs [`Capability::Network`].
    NetPcap,
}

/// Which way a network frame travelled, from the board's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetDirection {
    /// Sent by the board's GEM.
    Tx,
    /// Delivered to the board's GEM.
    Rx,
}

/// One captured Ethernet frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetFrame {
    /// Position in the capture, counted from the start of the run.
    pub index: u64,
    /// Virtual time the frame crossed the wire, in nanoseconds.
    pub t_ns: u64,
    /// Direction.
    pub dir: NetDirection,
    /// `arp`, `icmp`, `udp`, `tcp`, `ipv4`, `ipv6` or `other`.
    pub kind: String,
    /// One-line summary.
    pub summary: String,
    /// False when an IPv4 header or ICMP checksum does not verify.
    pub checksums_ok: bool,
    /// The frame bytes (no FCS).
    pub data: Vec<u8>,
}

/// Frame counters and the network's configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetStatus {
    /// MAC address given to GEM0.
    pub mac: String,
    /// The simulated LAN, `a.b.c.d/len`.
    pub subnet: String,
    /// The LAN's router/host/DHCP address.
    pub gateway: String,
    /// True when the guest is isolated from the machine's real network.
    pub restricted: bool,
    /// Frames the board sent.
    pub frames_tx: u64,
    /// Frames delivered to the board.
    pub frames_rx: u64,
    /// Bytes the board sent.
    pub bytes_tx: u64,
    /// Bytes delivered to the board.
    pub bytes_rx: u64,
    /// ARP frames, both directions.
    pub arp: u64,
    /// ICMP frames, both directions.
    pub icmp: u64,
    /// UDP frames, both directions.
    pub udp: u64,
    /// TCP frames, both directions.
    pub tcp: u64,
    /// Everything else.
    pub other: u64,
    /// Frames whose IPv4 header or ICMP checksum did not verify.
    pub bad_checksums: u64,
}

/// Simulation status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Current simulation time in picoseconds.
    pub now_ps: u64,
    /// True while continuously running.
    pub playing: bool,
    /// Lock-step quantum in picoseconds.
    pub quantum_ps: u64,
    /// PRNG seed in effect since the last reset.
    pub seed: u64,
    /// Board machine name.
    pub board: String,
    /// Active PS backend.
    pub ps_backend: String,
    /// Active PL backend.
    pub pl_backend: String,
    /// Fidelity of the active backend set.
    pub fidelity: Fidelity,
    /// What the active backend set can do.
    pub capabilities: Vec<Capability>,
    /// Number of trace records.
    pub trace_len: usize,
    /// Digest of the trace so far.
    pub trace_hash: String,
    /// Events waiting in the scheduler.
    pub pending_events: usize,
    /// Devices attached to Pmod ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<DeviceStatus>,
    /// Debugger state, when the engine was started with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugStatus>,
    /// Audio and video activity, when the PL carries the board's media models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaStatus>,
    /// The SD card and QSPI flash attached to the board, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageStatus>,
}

/// What is attached to the board's microSD slot and QSPI flash socket.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageStatus {
    /// The card in the microSD slot.
    pub sd: Option<StorageDevice>,
    /// The QSPI flash.
    pub qspi: Option<StorageDevice>,
}

/// One attached storage device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageDevice {
    /// Where the contents come from: an image file name, or `dir:NAME` for a
    /// host folder presented as a card.
    pub source: String,
    /// Size of the device in bytes.
    pub bytes: u64,
    /// The simulated part (QEMU device model name).
    pub model: String,
    /// True when guest writes reach the host file itself. Otherwise they go to
    /// a throw-away overlay and the host file is never modified.
    pub writable: bool,
}

/// What the media models have produced so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaStatus {
    /// Complete HDMI TX frames seen.
    pub video_frames: u32,
    /// Size of the newest frame, if any.
    pub video_size: Option<(u32, u32)>,
    /// Stereo audio frames received from the codec DAC.
    pub audio_frames: u64,
    /// Peak absolute sample of the retained audio.
    pub audio_peak: i32,
    /// Codec register writes seen over I2C.
    pub codec_writes: u32,
}

/// The answer to a [`Command`] or [`Query`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The command succeeded and there is nothing to report.
    Ok,
    /// The simulation time after a run command.
    Time {
        /// Current time in picoseconds.
        now_ps: u64,
    },
    /// Status.
    Status(Box<Status>),
    /// The board description.
    BoardDef(Box<BoardDef>),
    /// Board state.
    BoardState(BoardState),
    /// A pin level.
    Pin {
        /// The name that was looked up.
        name: String,
        /// Its level.
        level: PinLevel,
    },
    /// UART bytes.
    Uart {
        /// The bytes read.
        data: Vec<u8>,
        /// Offset to pass as `from` next time.
        next: u64,
        /// Offset of the oldest byte still retained.
        oldest: u64,
    },
    /// A page of trace records.
    Trace {
        /// The records.
        records: Vec<TraceRecord>,
        /// Index to pass as `from` next time.
        next: usize,
        /// Total records in the trace.
        total: usize,
        /// Digest of the whole trace.
        hash: String,
    },
    /// Exported trace text (VCD or JSON).
    Text(String),
    /// The signal catalogue of the trace.
    Signals {
        /// The signals, in name order.
        signals: Vec<trace::waveform::SignalInfo>,
        /// Time of the newest record in picoseconds.
        now_ps: u64,
        /// Records ever recorded; changes when the catalogue may have.
        trace_len: usize,
    },
    /// Value changes of chosen signals in a time window.
    Waveform(Box<trace::waveform::Waveform>),
    /// A recording.
    Recording(Box<crate::recording::Recording>),
    /// Snapshot names.
    Snapshots(Vec<String>),
    /// PNG bytes.
    Png(Vec<u8>),
    /// The newest complete HDMI TX frame.
    Video {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Frame number since the start of the run.
        frame: u32,
        /// PNG-encoded picture.
        png: Vec<u8>,
    },
    /// Codec DAC audio.
    Audio {
        /// Sample rate in hertz.
        sample_rate: u32,
        /// Index of the first returned stereo frame since the start of the run.
        first_frame: u64,
        /// Number of stereo frames returned.
        frames: u64,
        /// Peak absolute sample value.
        peak: i32,
        /// RMS level, in sample units.
        rms: f64,
        /// Dominant frequency by zero crossings of the left channel, 0 if none.
        freq_hz: f64,
        /// WAV file bytes.
        wav: Vec<u8>,
    },
    /// Registers of a stopped core.
    CpuRegs(CpuRegs),
    /// The breakpoints and watchpoints that are set.
    Breakpoints(Vec<BreakpointInfo>),
    /// The picture of a device.
    DeviceImage {
        /// Pmod port index.
        port: u8,
        /// Device kind.
        kind: String,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// PNG-encoded picture.
        png: Vec<u8>,
        /// One line of the device's state.
        description: String,
    },
    /// The state of a device.
    DeviceState {
        /// Pmod port index.
        port: u8,
        /// Device kind.
        kind: String,
        /// One line of the device's state.
        description: String,
        /// The quantities the host can set, with their current values.
        values: Vec<DeviceValueInfo>,
        /// The device's registers.
        registers: Vec<DeviceRegisterInfo>,
    },
    /// Disassembled instructions.
    Disasm {
        /// Whether Thumb decoding was used.
        thumb: bool,
        /// The instructions.
        lines: Vec<DisasmLine>,
    },
    /// A resolved symbol.
    Symbol(SymbolInfo),
    /// Guest memory contents.
    Memory {
        /// Physical address of the first byte.
        addr: u64,
        /// The bytes.
        data: Vec<u8>,
    },
    /// A page of captured Ethernet frames.
    Net {
        /// The frames.
        frames: Vec<NetFrame>,
        /// Index to pass as `from` next time.
        next: u64,
        /// Frames captured so far.
        total: u64,
        /// Index of the oldest frame still retained.
        oldest: u64,
    },
    /// Ethernet counters.
    NetStats(NetStatus),
    /// A `.pcap` file.
    Pcap(Vec<u8>),
}

/// A notification from the simulation thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A new trace record was appended.
    Trace {
        /// Index of the record in the trace.
        index: usize,
        /// The record.
        record: TraceRecord,
    },
    /// Continuous running started or stopped.
    Playing {
        /// New state.
        playing: bool,
    },
    /// State was replaced wholesale (snapshot restore); re-query everything.
    Restored,
    /// The engine failed while running continuously; running has stopped.
    Fault {
        /// What went wrong.
        message: String,
    },
}
