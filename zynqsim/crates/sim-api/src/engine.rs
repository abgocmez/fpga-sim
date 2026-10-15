use crate::error::ApiError;
use crate::sim::Simulator;
use crate::types::{Command, Query, Response, Status};
use board_zybo::{BoardDef, BoardState};
use sim_core::Ps;
use trace::TraceLog;

/// Whatever sits behind the control plane: the built-in demo model, or a PS
/// engine with a PL attached.
///
/// The server thread owns one `Engine` and is the only caller, so
/// implementations need not be internally synchronised, only `Send`.
pub trait Engine: Send {
    /// The board description.
    fn def(&self) -> &BoardDef;
    /// The board's user-visible state.
    fn board(&self) -> &BoardState;
    /// The event trace so far.
    fn trace(&self) -> &TraceLog;
    /// Status, including the fidelity and capabilities of this engine.
    fn status(&self, playing: bool) -> Status;
    /// Executes a state-changing command. `Play` and `Pause` are handled by
    /// the server and never reach the engine.
    fn command(&mut self, cmd: Command) -> Result<Response, ApiError>;
    /// Answers a query. It does not change simulated state, but may need the
    /// engine's connections (memory reads go through the PS).
    fn query(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError>;
    /// Advances by `d`; used by the server for continuous running.
    fn run_for(&mut self, d: Ps) -> Result<(), ApiError>;
}

impl Engine for Simulator {
    fn def(&self) -> &BoardDef {
        Simulator::def(self)
    }
    fn board(&self) -> &BoardState {
        Simulator::board(self)
    }
    fn trace(&self) -> &TraceLog {
        Simulator::trace(self)
    }
    fn status(&self, playing: bool) -> Status {
        Simulator::status(self, playing)
    }
    fn command(&mut self, cmd: Command) -> Result<Response, ApiError> {
        Simulator::command(self, cmd)
    }
    fn query(&mut self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        Simulator::query(self, q, playing)
    }
    fn run_for(&mut self, d: Ps) -> Result<(), ApiError> {
        Simulator::run_for(self, d);
        Ok(())
    }
}
