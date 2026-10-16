//! `boot_inspect`: a Zynq-7000 BOOT.BIN, read but not run (ADR-018).

use super::{McpServer, Out, str_arg};
use serde_json::Value;
use sim_api::ApiError;

impl McpServer {
    /// Partitions, register-init table, findings and whether the image can boot.
    pub(super) fn boot_inspect(&mut self, a: &Value) -> Result<Out, ApiError> {
        let file = self.resolve(str_arg(a, "path")?, true)?;
        Ok(Out::json(sim_api::bootbin::inspect(&file)?))
    }
}
