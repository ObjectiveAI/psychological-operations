//! Per-session state. Populated by
//! [`crate::header_session_manager::HeaderSessionManager`] from the
//! `X-OBJECTIVEAI-ARGUMENTS` header, consumed by every tool handler via
//! [`super::PsychologicalOperationsTwitchMcp::resolve_session`].
//!
//! Identical in shape to the Discord MCP's session state — `tag`, `mode`, and
//! the per-session quota overrides — since the Twitch MCP takes the same
//! session arguments. Two differences: the per-tool `quota_usage_<tool>` set
//! (the Twitch tools), and the `max_message_length` default (Twitch's 500, not
//! Discord's 2000). In-memory only; the header is re-sent on every connect so a
//! process restart is transparent.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use super::tool_name::ToolName;
use crate::Mode;

/// HTTP header objectiveai stamps with a JSON object of per-URL arguments. We
/// look for `tag`, `mode`, `max_message_length`, and the `quota_*` keys
/// case-insensitively.
pub const HEADER_ARGUMENTS: &str = "X-OBJECTIVEAI-ARGUMENTS";

/// Default `max_message_length` (Twitch's standard chat char limit) when the
/// `max_message_length` argument is absent.
pub const DEFAULT_MAX_MESSAGE_LENGTH: u64 = 500;

/// The values pulled from the request HTTP headers and pinned to the rmcp
/// session in memory.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// The agent tag this session acts as (REQUIRED `tag` argument) — the
    /// identity every tool authenticates as, and the quota-ledger key.
    pub tag: String,
    pub mode: Mode,
    /// Max characters a sent message may have (`max_message_length`; defaults
    /// to [`DEFAULT_MAX_MESSAGE_LENGTH`]). Enforced by `send_message` before
    /// hitting Twitch.
    pub max_message_length: usize,
    /// Per-session read-budget limit (`quota_read`; default-backed).
    pub quota_read: u64,
    /// Per-session write-budget limit (`quota_write`; default-backed).
    pub quota_write: u64,
    /// Per-session quota window in SECONDS (`quota_interval`; default-backed).
    pub quota_interval: u64,
    /// Per-session per-tool cost overrides (`quota_usage_<tool>`), one entry
    /// per metered tool.
    pub quota_tool_costs: HashMap<ToolName, u64>,
}

/// In-memory map of `SessionId → SessionState`.
#[derive(Default, Debug, Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<HashMap<SessionId, Arc<SessionState>>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn record(&self, id: SessionId, state: Arc<SessionState>) {
        self.inner.write().await.insert(id, state);
    }

    pub async fn get(&self, id: &SessionId) -> Option<Arc<SessionState>> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &SessionId) -> Option<Arc<SessionState>> {
        self.inner.write().await.remove(id)
    }
}
