use crate::error::ApiError;
use crate::types::{Command, Input};
use serde::{Deserialize, Serialize};

/// Everything needed to construct a simulator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimConfig {
    /// Built-in board machine name.
    pub board: String,
    /// Initial PRNG seed.
    pub seed: u64,
    /// Lock-step quantum in picoseconds (non-zero).
    pub quantum_ps: u64,
}

impl Default for SimConfig {
    fn default() -> SimConfig {
        SimConfig {
            board: "zybo-z7-20".to_string(),
            seed: 1,
            quantum_ps: 1_000_000, // 1 us, the default of prompt §4
        }
    }
}

/// An input and the simulation time it was applied at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecEntry {
    /// Simulation time in picoseconds.
    pub t_ps: u64,
    /// The input.
    pub input: Input,
}

/// A session: configuration, time-stamped inputs and the resulting digest.
///
/// Replaying the entries against a fresh simulator must reproduce
/// `trace_hash` (requirement §4: deterministic replay of recorded sessions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// Format version; currently 1.
    pub version: u32,
    /// Simulator configuration at the start of the session.
    pub config: SimConfig,
    /// Inputs in application order.
    pub entries: Vec<RecEntry>,
    /// Simulation time at the end of the session, in picoseconds.
    pub end_ps: u64,
    /// Trace digest at the end of the session.
    pub trace_hash: String,
    /// QEMU sessions: every state-changing request in order, with the run
    /// targets. Replaying a QEMU session needs exactly this sequence (UART
    /// bytes are attributed to the chunk they were seen in).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub journal: Vec<Command>,
    /// QEMU sessions: the `project.toml` the session was built from. Set by the
    /// front end that opened the project; replay rebuilds the engine from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// The outcome of replaying a [`Recording`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Digest produced by the replay.
    pub trace_hash: String,
    /// Digest stored in the recording.
    pub expected_hash: String,
    /// True when the two agree.
    pub matches: bool,
    /// Simulation time reached.
    pub end_ps: u64,
    /// Number of trace records produced.
    pub trace_len: usize,
}

/// Replays a QEMU session: rebuilds the engine from the project named in the
/// recording, applies the journal and compares the trace digest.
pub fn replay_recording(rec: &Recording) -> Result<ReplayReport, ApiError> {
    replay_engine(rec).map(|(_, report)| report)
}

/// Like [`replay_recording`], also returning the engine that was replayed (for
/// exporting its trace).
pub fn replay_engine(rec: &Recording) -> Result<(Box<dyn crate::Engine>, ReplayReport), ApiError> {
    use crate::Project;
    let file = rec.project.as_deref().ok_or_else(|| {
        ApiError::invalid_argument(
            "this recording has no `project`: it cannot be replayed without the project.toml \
             it was recorded from",
        )
    })?;
    let path = std::path::Path::new(file);
    let project = Project::load(path)?;
    let base = path.parent().unwrap_or(std::path::Path::new("."));
    let mut engine = project.build_engine(base)?;
    for c in &rec.journal {
        engine.command(c.clone())?;
    }
    let st = engine.status(false);
    let report = ReplayReport {
        matches: st.trace_hash == rec.trace_hash && st.now_ps == rec.end_ps,
        trace_hash: st.trace_hash,
        expected_hash: rec.trace_hash.clone(),
        end_ps: st.now_ps,
        trace_len: st.trace_len,
    };
    Ok((engine, report))
}
