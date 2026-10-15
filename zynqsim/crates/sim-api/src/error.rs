use serde::{Deserialize, Serialize};

/// Machine-readable error category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A parameter is malformed or out of range.
    InvalidArgument,
    /// A named object (pin, snapshot, file) does not exist.
    NotFound,
    /// The active backend or this build cannot do that. Never a silent no-op.
    Unsupported,
    /// A file or OS operation failed.
    Io,
    /// The simulation thread is gone.
    Disconnected,
    /// The caller cancelled a long-running operation.
    Cancelled,
    /// A bug in the simulator.
    Internal,
}

/// A structured API error, carried unchanged through CLI, GUI and MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ApiError {
    /// Category.
    pub code: ErrorCode,
    /// What went wrong, in one or two sentences.
    pub message: String,
    /// What the caller can do about it, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ApiError {
    fn new(code: ErrorCode, message: impl Into<String>) -> ApiError {
        ApiError {
            code,
            message: message.into(),
            hint: None,
        }
    }

    /// A malformed or out-of-range parameter.
    pub fn invalid_argument(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::InvalidArgument, message)
    }

    /// A missing named object.
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::NotFound, message)
    }

    /// A feature the active backend or this build does not provide.
    pub fn unsupported(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::Unsupported, message)
    }

    /// An I/O failure.
    pub fn io(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::Io, message)
    }

    /// The simulation thread has terminated.
    pub fn disconnected() -> ApiError {
        ApiError::new(
            ErrorCode::Disconnected,
            "the simulation thread is not running",
        )
    }

    /// The caller cancelled the operation.
    pub fn cancelled(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::Cancelled, message)
    }

    /// An internal invariant was violated.
    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::Internal, message)
    }

    /// Attaches a remediation hint.
    pub fn with_hint(mut self, hint: impl Into<String>) -> ApiError {
        self.hint = Some(hint.into());
        self
    }
}
