//! Per-session state. Populated by
//! [`crate::header_session_manager::HeaderSessionManager`] when the
//! initialize request lands; consumed by every tool handler via
//! [`super::PsychologicalOperationsXApiMcp::resolve_session`].
//!
//! Only the two values the client supplies in headers belong here:
//! `agent` and `mode`. Everything else (`config_base_dir`,
//! `cache_max_size`, `cache_ttl`) is process-wide and lives on the
//! server struct — so when the operator restarts the binary with
//! a different cache budget, every active session (including ones
//! restored from disk, once persistence lands) picks up the new
//! value automatically.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use crate::Mode;

/// HTTP header the client must send on initialize to bind a session
/// to an agent. Read once, persisted with the session.
pub const HEADER_AGENT: &str = "X-PSYOP-X-API-AGENT";

/// HTTP header the client must send on initialize to choose the
/// tool surface. `"readonly"` or `"full"`. Read once, persisted
/// with the session.
pub const HEADER_MODE: &str = "X-PSYOP-X-API-MODE";

/// The two values pulled from the initial HTTP headers and pinned
/// to the rmcp session for its lifetime.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub agent: String,
    pub mode: Mode,
}

/// In-memory map of `SessionId → SessionState`, shared between the
/// custom session manager (which records on initialize, drops on
/// close) and the tool handlers (which look up on every call).
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
