//! Per-session state. Populated by
//! [`crate::header_session_manager::HeaderSessionManager`] when the
//! initialize request lands (either through the no-session-id
//! initialize path, or via lazy capture on a reconnect with an
//! id we haven't seen this process lifetime). Consumed by every
//! tool handler via
//! [`super::PsychologicalOperationsXApiMcp::resolve_session`].
//!
//! Only the values the client supplies belong here: `agent`,
//! `mode`, and the agent instance hierarchy (the quota-ledger
//! key). Everything else (`state_dir`, `cache_max_size`,
//! `cache_ttl`, the quota limits) is process-wide and lives on
//! the server struct.
//!
//! ## Where `agent` and `mode` come from
//!
//! objectiveai no longer forwards arbitrary custom client headers.
//! Instead it stamps two headers on every outbound request:
//!
//!   - [`HEADER_ARGUMENTS`] (`X-OBJECTIVEAI-ARGUMENTS`) carries a
//!     JSON object of per-URL key/value pairs the upstream client
//!     wanted to pass. We do a case-insensitive lookup for `agent`
//!     and `mode` keys here.
//!   - [`HEADER_AGENT_INSTANCE_HIERARCHY`]
//!     (`X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY`) is the
//!     session-global agent-id chain objectiveai maintains. If the
//!     arguments map doesn't carry an `agent`, we fall back to this
//!     header.
//!
//! `agent` is required; missing from both sources is a hard error.
//! `mode` is optional; missing defaults to [`Mode::Readonly`].
//!
//! ## In-memory only
//!
//! The registry is in-memory only. objectiveai re-sends these
//! headers on every connect, so a process restart that flushes
//! this map is invisible to the client: the lazy header capture in
//! [`crate::header_session_manager::HeaderSessionManager::create_stream`]
//! rebuilds the entry from the next request's headers.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use crate::Mode;

/// HTTP header objectiveai stamps with a JSON object of per-URL
/// arguments. We look for `agent` and `mode` keys
/// case-insensitively. See module docs.
pub const HEADER_ARGUMENTS: &str = "X-OBJECTIVEAI-ARGUMENTS";

/// HTTP header objectiveai stamps with the session-global agent
/// instance hierarchy. Used as the fallback source for `agent`
/// when the arguments map doesn't carry one.
pub const HEADER_AGENT_INSTANCE_HIERARCHY: &str = "X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY";

/// The values pulled from the request HTTP headers and pinned
/// to the rmcp session in memory.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub agent: String,
    pub mode: Mode,
    /// Session-global agent-id chain from
    /// [`HEADER_AGENT_INSTANCE_HIERARCHY`]; falls back to the
    /// resolved `agent` when the header is absent. Keys the
    /// per-caller API request log (and thus the read/write
    /// quota ledger).
    pub agent_instance_hierarchy: String,
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
