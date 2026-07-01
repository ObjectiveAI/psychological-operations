//! Classifying a tool-call failure into one of two buckets:
//!
//! * **System fault** — infra / setup / credentials broke; not the agent's
//!   doing. Surfaced as `Err(ErrorData)` (a JSON-RPC protocol error).
//! * **Agent fault** — the tool-call inputs were wrong, or the authorized
//!   request was rejected for a reason the agent can act on. Surfaced as a
//!   `CallToolResult { is_error: true }` so the model reads it and
//!   self-corrects.
//!
//! Mirrors the Discord MCP's `tool_error`, minus the serenity `From` impl (the
//! Twitch client surfaces its request outcomes through the `Http` variant).

use psychological_operations_sdk::twitch;
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

impl From<twitch::Error> for ToolError {
    /// Classify a Twitch client error: missing/unusable credentials, a db read,
    /// or a cache (de)serialize is infra (system); the authorized Helix
    /// request's own `Http` outcome (bad status, dropped message) is
    /// agent-actionable.
    fn from(e: twitch::Error) -> Self {
        let msg = e.to_string();
        match e {
            twitch::Error::NotAuthed(_) | twitch::Error::Db(_) | twitch::Error::Serde(_) => {
                ToolError::System(ErrorData::internal_error(msg, None))
            }
            twitch::Error::Http(_) => ToolError::Agent(msg),
        }
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
