//! Per-session state. Populated by
//! [`crate::header_session_manager::HeaderSessionManager`] when the
//! initialize request lands (either through the no-session-id
//! initialize path, or via lazy capture on a reconnect with an
//! id we haven't seen this process lifetime). Consumed by every
//! tool handler via
//! [`super::PsychologicalOperationsXApiMcp::resolve_session`].
//!
//! Only the two values the client supplies in headers belong here:
//! `agent` and `mode`. Everything else (`config_base_dir`,
//! `cache_max_size`, `cache_ttl`) is process-wide and lives on the
//! server struct.
//!
//! The registry is in-memory only. The CLI re-sends the
//! `X-PSYOP-X-API-{AGENT,MODE}` headers on every connect, so a
//! process restart that flushes this map is invisible to the
//! client: the lazy header capture in
//! [`crate::header_session_manager::HeaderSessionManager::create_stream`]
//! rebuilds the entry from the next request's headers.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use crate::Mode;

/// HTTP header the client sends on every connect to bind a session
/// to an agent. Read on initial connect (recorded by
/// `initialize_session`) AND on any subsequent request that lands
/// for a session id we don't currently hold in memory (re-captured
/// by the manager's lazy `ensure_session` path).
pub const HEADER_AGENT: &str = "X-PSYOP-X-API-AGENT";

/// HTTP header the client sends on every connect to choose the
/// tool surface. `"readonly"` or `"full"`. Same lifecycle as
/// [`HEADER_AGENT`].
pub const HEADER_MODE: &str = "X-PSYOP-X-API-MODE";

/// The two values pulled from the request HTTP headers and pinned
/// to the rmcp session in memory.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub agent: String,
    pub mode: Mode,
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
