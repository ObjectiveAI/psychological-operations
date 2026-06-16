//! Per-session state. Populated by
//! [`crate::header_session_manager::HeaderSessionManager`] when the
//! initialize request lands (either through the no-session-id
//! initialize path, or via lazy capture on a reconnect with an
//! id we haven't seen this process lifetime). Consumed by every
//! tool handler via
//! [`super::PsychologicalOperationsXApiMcp::resolve_session`].
//!
//! Only the values the client supplies belong here: `account`, `mode`,
//! and the per-session quota overrides. Everything else (`state_dir`,
//! `cache_max_size`, `cache_ttl`) is process-wide and lives on the
//! server struct.
//!
//! ## Where these come from
//!
//! objectiveai stamps [`HEADER_ARGUMENTS`] (`X-OBJECTIVEAI-ARGUMENTS`)
//! on every outbound request: a JSON object of per-URL key/value pairs
//! the upstream client passed. We do case-insensitive lookups for
//! `account` (REQUIRED — the X identity to act as), `mode` (REQUIRED),
//! and the optional `quota_*` overrides. A missing/malformed `account`
//! or `mode` is a hard error; the quota args fall back to the process
//! defaults.
//!
//! ## In-memory only
//!
//! The registry is in-memory only. objectiveai re-sends the header on
//! every connect, so a process restart that flushes this map is
//! invisible to the client: the lazy header capture in
//! [`crate::header_session_manager::HeaderSessionManager::create_stream`]
//! rebuilds the entry from the next request's headers.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use crate::Mode;
use super::tool_name::ToolName;

/// HTTP header objectiveai stamps with a JSON object of per-URL
/// arguments. We look for `account`, `mode`, and the `quota_*` keys
/// case-insensitively. See module docs.
pub const HEADER_ARGUMENTS: &str = "X-OBJECTIVEAI-ARGUMENTS";

/// The values pulled from the request HTTP headers and pinned
/// to the rmcp session in memory.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// The X account this session acts as, from the client's REQUIRED
    /// `account` argument — the identity every tool authenticates as,
    /// and the key the quota ledger is charged against.
    pub account: String,
    pub mode: Mode,
    /// Per-session read-budget limit (`quota_read`; falls back to the
    /// process default). Charged against by read tools in `enforce_quota`.
    pub quota_read: u64,
    /// Per-session write-budget limit (`quota_write`; default-backed).
    pub quota_write: u64,
    /// Per-session quota window in SECONDS (`quota_interval`;
    /// default-backed).
    pub quota_interval: u64,
    /// Per-session per-tool cost overrides (`quota_usage_<tool>`), one
    /// entry per metered tool, each falling back to the default cost.
    pub quota_tool_costs: HashMap<ToolName, u64>,
}

/// In-memory map of `SessionId → SessionState`. Shared between the
/// custom session manager (which records on initialize / lazy
/// capture, drops on close) and the tool handlers (which look up
/// on every call via the Mcp-Session-Id header).
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
