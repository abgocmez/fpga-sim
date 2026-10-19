//! The ZynqSim control plane.
//!
//! Typed commands, queries and an event stream (prompt §13). The GUI, CLI and
//! MCP server are thin front ends over this crate and depend on nothing else
//! in the workspace; the board and trace types they need are re-exported here.
//!
//! In milestone P0 the engine behind the API is the built-in *demo* model
//! (see [`sim`]); every status reports it as [`Fidelity::Demo`].

pub mod bootbin;
mod bytelog;
mod disasm;
mod duration;
mod elf;
mod engine;
mod error;
pub mod exportinit;
pub mod exportsim;
mod native_pl;
mod native_ps;
mod netlog;
mod pl_attach;
mod project;
pub mod protocol;
pub mod ps7_init;
mod qemu_engine;
mod recording;
mod server;
pub mod sim;
mod source;
pub mod storage;
mod tbgen;
mod tools;
mod types;
mod wave;
pub mod xsa;

pub use duration::parse_duration;
pub use engine::Engine;
pub use error::{ApiError, ErrorCode};
pub use native_ps::{BATCH_CYCLES, NativePsEngine, NativeSpec, PS_PER_CYCLE};
pub use pl_attach::{RtlNativeSlaveSpec, RtlNativeSpec};
pub use project::{
    BackendsSection, BoardSection, BootSection, CpuSection, DebugSection, Diagnostic, MediaSection,
    NetworkSection, Project, ProtocolCheckers, RtlSection, Severity, SimSection, StorageSection,
    parse_audio_spec, parse_video_spec, starter_project_toml,
};
pub use protocol::{ProtocolSummary, protocol_summary};
pub use qemu_engine::{
    ImageSource, NATIVE_MODELS, NativeInstance, QemuEngine, QemuSpec, RtlSpec, RtlTool, StorageSpec,
};
pub use recording::{
    RecEntry, Recording, ReplayReport, SimConfig, replay_engine, replay_recording,
};
pub use server::SimClient;
pub use sim::Simulator;
pub use tbgen::{
    AxiLite, AxiMaster, Dir, ModuleInfo, PROTOCOL_CHECKER_FILE, PinGroup, Port, SlaveDesc,
    detect_axi_lite, detect_axi_master, detect_irqs, detect_pin_groups,
    generate_multi_slave_testbench, generate_project, generate_testbench, master_bridge_with_dev,
    master_interface_device, parse_module, protocol_checker_source, relative_path,
};
pub use tools::{
    BackendInfo, BackendKind, BackendRole, DoctorReport, DoctorSummary, HostInfo, RouteStatus,
    ToolStatus, detect_backends, diagnose, find_dpi_lib, find_on_path, find_vivado, locate,
    locate_binary,
};
pub use types::{
    AudioSpec, BreakKind, BreakpointInfo, Capability, Command, CpuRegs, DebugStatus,
    DeviceRegisterInfo, DeviceStatus, DeviceValueInfo, DisasmLine, Event, Fidelity, Input,
    MediaStatus, NetDirection, NetFrame, NetStatus, Query, Response, SourceLine, SourceRef, Status,
    StorageDevice, StorageStatus, SymbolInfo, TraceFormat, VideoSpec,
};

/// Board definition, state, rendering and hit-testing.
pub use board_zybo as board;
pub use devices::font5x7;
/// Device kinds `[devices]` and `pmod_attach` accept, and the names of the
/// quantities a kind lets the host set.
pub use devices::{KINDS as DEVICE_KINDS, value_names as device_value_names};
pub use pl_api::media::{SAMPLE_RATE as AUDIO_SAMPLE_RATE, parse_wav};
/// Picosecond time type.
pub use sim_core::Ps;
/// Trace records and exporters.
pub use trace;
