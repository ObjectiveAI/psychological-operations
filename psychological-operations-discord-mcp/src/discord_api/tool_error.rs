//! Classifying a tool-call failure into one of two buckets:
//!
//! * **System fault** — infra / setup / credentials broke; not the agent's
//!   doing. Surfaced as `Err(ErrorData)` (a JSON-RPC protocol error).
//! * **Agent fault** — the tool-call inputs were wrong, or the authorized
//!   request was rejected for a reason the agent can act on. Surfaced as a
//!   `CallToolResult { is_error: true }` so the model reads it and
//!   self-corrects.
//!
//! Mirrors the X MCP's `tool_error`, minus the X-client `From` impl (the
//! Discord-client `From` will be added with the read/write tools).

use psychological_operations_sdk::discord::{self, serenity};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};

/// A tool failure tagged with how it should surface (see module docs).
pub enum ToolError {
    /// Infra/setup/credentials — emit as a protocol error.
    System(ErrorData),
    /// Agent-actionable — emit as an `is_error` tool result.
    Agent(String),
}

impl ToolError {
    /// An agent-facing failure carrying a message the model should read.
    pub fn agent(msg: impl Into<String>) -> Self {
        ToolError::Agent(msg.into())
    }
}

impl From<serde_json::Error> for ToolError {
    /// Encoding a tool's own response body — should never fail; internal.
    fn from(e: serde_json::Error) -> Self {
        ToolError::System(ErrorData::internal_error(format!("serialize: {e}"), None))
    }
}

impl From<psychological_operations_db::Error> for ToolError {
    /// A persistence-layer failure (postgres) — infra, not the agent's doing.
    fn from(e: psychological_operations_db::Error) -> Self {
        ToolError::System(ErrorData::internal_error(format!("db: {e}"), None))
    }
}

impl From<discord::Error> for ToolError {
    /// Classify a Discord client error: a missing/unusable bot token or a db
    /// read is infra (system); an actual serenity API outcome is the
    /// authorized request's own result (agent-actionable).
    fn from(e: discord::Error) -> Self {
        let msg = e.to_string();
        match e {
            discord::Error::NotAuthed(_) | discord::Error::Db(_) | discord::Error::Serde(_) => {
                ToolError::System(ErrorData::internal_error(msg, None))
            }
            discord::Error::Serenity(_) => ToolError::Agent(msg),
        }
    }
}

impl From<serenity::Error> for ToolError {
    /// The authorized Discord request's own outcome (permissions, not found,
    /// rate limit, …) — agent-actionable.
    fn from(e: serenity::Error) -> Self {
        ToolError::Agent(e.to_string())
    }
}

/// Collapse a tool body's `Result<_, ToolError>` into the rmcp contract: a
/// system fault becomes the `Err(ErrorData)` protocol error; an agent fault
/// becomes an `is_error` `CallToolResult` the model can read.
pub fn finish(r: Result<CallToolResult, ToolError>) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(ok) => Ok(ok),
        Err(ToolError::System(e)) => Err(e),
        Err(ToolError::Agent(msg)) => Ok(CallToolResult::error(vec![Content::text(msg)])),
    }
}
