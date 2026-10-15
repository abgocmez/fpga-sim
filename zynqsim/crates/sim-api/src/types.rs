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
    /// Audio and video activity, when the PL carries the board's media models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaStatus>,
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
