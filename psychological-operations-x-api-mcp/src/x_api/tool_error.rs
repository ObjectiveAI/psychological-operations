//! Classifying a tool-call failure into one of two buckets and rendering
//! it the right way:
//!
//! * **System fault** — infra / setup / credentials broke; not the
//!   agent's doing. Surfaced as `Err(ErrorData)` (a JSON-RPC protocol
//!   error / operator signal).
//! * **Agent fault** — the tool-call inputs were wrong, or the *authorized*
//!   X request itself was rejected for a reason the agent can act on (bad
//!   id, 403 replies-disabled, rate limit, …). Surfaced as a
//!   `CallToolResult { is_error: true }` so the model reads it and
//!   self-corrects.
//!
//! The X API is the special case: it's split by **call-path phase**, never
//! by HTTP status. Failures while *obtaining authorization*
//! ([`x::Error::Authorization`], tagged at the SDK's resolution boundary)
//! are system; failures of the *authorized request* (transport, status,
//! problem body) are agent. A 401/403 can come from either phase, so
//! status alone can't tell them apart — the SDK already did, by phase.

use psychological_operations_sdk::x::Error as XError;
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
    /// An agent-facing failure carrying a message the model should read
    /// (bad argument, missing precondition, …).
    pub fn agent(msg: impl Into<String>) -> Self {
        ToolError::Agent(msg.into())
    }
}

impl From<XError> for ToolError {
    /// Classify an X client error by call-path phase, not status:
    /// - **System**: `Authorization` (auth-resolution phase), `Db`
    ///   (response-cache postgres), `RequestBuild` (internal), `Deserialize`
    ///   (response schema drift — our side).
    /// - **Agent**: `Transport` / `BadStatus` / `Problem` / `Other` — the
    ///   authorized request's own outcome.
    fn from(e: XError) -> Self {
        let msg = e.to_string();
        match e {
            XError::Authorization(_)
            | XError::Db(_)
            | XError::RequestBuild(_)
            | XError::Deserialize(_) => {
                ToolError::System(ErrorData::internal_error(msg, None))
            }
            XError::Transport(_)
            | XError::BadStatus { .. }
            | XError::Problem { .. }
            | XError::Other(_) => ToolError::Agent(msg),
        }
    }
}

impl From<serde_json::Error> for ToolError {
    /// Encoding a tool's own response body — should never fail; treat as
    /// internal.
    fn from(e: serde_json::Error) -> Self {
        ToolError::System(ErrorData::internal_error(format!("serialize: {e}"), None))
    }
}

/// Collapse a tool body's `Result<_, ToolError>` into the rmcp contract:
/// a system fault becomes the `Err(ErrorData)` protocol error; an agent
/// fault becomes an `is_error` `CallToolResult` the model can read.
pub fn finish(r: Result<CallToolResult, ToolError>) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(ok) => Ok(ok),
        Err(ToolError::System(e)) => Err(e),
        Err(ToolError::Agent(msg)) => Ok(CallToolResult::error(vec![Content::text(msg)])),
    }
}
