//! MCP server for ZynqSim: lets AI agents build, run, observe and debug a
//! simulated Zybo Z7-20 over the Model Context Protocol (stdio, JSON-RPC 2.0,
//! newline-delimited).
//!
//! Safety properties (prompt §13): file access is confined to allow-listed
//! roots, no tool executes arbitrary commands, outputs are bounded, and long
//! operations report progress and honour `notifications/cancelled`.

mod schema;
mod server;
mod stdio;
mod tools;
mod util;

pub use schema::validate as validate_arguments;
pub use server::{Control, McpServer, NoControl};
pub use stdio::serve;
pub use tools::{Gate, ToolDef, tool_defs};
pub use util::{parse_hex, to_hex};
