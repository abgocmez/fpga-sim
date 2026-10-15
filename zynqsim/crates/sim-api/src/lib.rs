//! The ZynqSim control plane.
//!
//! Typed commands, queries and an event stream (prompt §13). The GUI, CLI and
//! MCP server are thin front ends over this crate and depend on nothing else
//! in the workspace; the board and trace types they need are re-exported here.
//!
//! In milestone P0 the engine behind the API is the built-in *demo* model
//! (see [`sim`]); every status reports it as [`Fidelity::Demo`].

mod bytelog;
mod disasm;
mod duration;
mod elf;
mod engine;
mod error;
mod project;
mod qemu_engine;
mod recording;
mod server;
pub mod sim;
mod tbgen;
mod tools;
mod types;

pub use duration::parse_duration;
pub use engine::Engine;
pub use error::{ApiError, ErrorCode};
pub use project::{
    BackendsSection, BoardSection, BootSection, CpuSection, DebugSection, Diagnostic, MediaSection,
    Project, RtlSection, Severity, SimSection, parse_audio_spec, parse_video_spec,
    starter_project_toml,
};
pub use qemu_engine::{NATIVE_MODELS, NativeInstance, QemuEngine, QemuSpec, RtlSpec};
pub use recording::{
    RecEntry, Recording, ReplayReport, SimConfig, replay_engine, replay_recording,
};
pub use server::SimClient;
pub use sim::Simulator;
pub use tbgen::{
    AxiLite, AxiMaster, Dir, ModuleInfo, PinGroup, Port, detect_axi_lite, detect_axi_master,
    detect_irqs, detect_pin_groups, generate_project, generate_testbench, parse_module,
    relative_path,
};
pub use tools::{
    BackendInfo, BackendKind, BackendRole, detect_backends, find_dpi_lib, find_on_path,
    find_vivado, locate, locate_binary,
};
pub use types::{
    AudioSpec, BreakKind, BreakpointInfo, Capability, Command, CpuRegs, DebugStatus, DeviceStatus,
    DisasmLine, Event, Fidelity, Input, MediaStatus, Query, Response, Status, SymbolInfo,
    TraceFormat, VideoSpec,
};

/// Board definition, state, rendering and hit-testing.
pub use board_zybo as board;
pub use devices::font5x7;
pub use pl_api::media::{SAMPLE_RATE as AUDIO_SAMPLE_RATE, parse_wav};
/// Picosecond time type.
pub use sim_core::Ps;
/// Trace records and exporters.
pub use trace;
