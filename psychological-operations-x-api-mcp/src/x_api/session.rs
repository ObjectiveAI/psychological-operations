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
//! restored from disk) picks up the new value automatically.
//!
//! The registry is disk-backed. The on-disk store lives in the
//! SDK at
//! [`psychological_operations_sdk::x_api_mcp_sessions::SessionStore`]
//! — one sqlite file under
//! `<config_base_dir>/plugins/psychological-operations/x-api-mcp-sessions.sqlite`.
//! Every mutator on the registry write-throughs to disk, and
//! [`SessionRegistry::open`] hydrates the in-memory map from disk
//! on startup. Tool handlers (`get`) hit memory only.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use psychological_operations_sdk::x::Error as SdkError;
use psychological_operations_sdk::x_api_mcp_sessions::{SessionRecord, SessionStore};
use rmcp::transport::common::server_side_http::SessionId;
use tokio::sync::RwLock;

use crate::Mode;

/// HTTP header the client must send on the *initial* connect to
/// bind a session to an agent. Read once, persisted with the
/// session; subsequent reconnects ignore it and use the
/// saved value.
pub const HEADER_AGENT: &str = "X-PSYOP-X-API-AGENT";

/// HTTP header the client must send on the *initial* connect to
/// choose the tool surface. `"readonly"` or `"full"`. Read once,
/// persisted with the session.
pub const HEADER_MODE: &str = "X-PSYOP-X-API-MODE";

/// The two values pulled from the initial HTTP headers and pinned
/// to the rmcp session for its lifetime — including across server
/// restart.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub agent: String,
    pub mode: Mode,
}

/// In-memory map of `SessionId → SessionState` mirrored to a sqlite
/// store. Lives both in the custom session manager (which records
/// on initialize, drops on close) and on the tool handlers (which
/// look up on every call).
#[derive(Debug, Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<HashMap<SessionId, Arc<SessionState>>>>,
    store: Arc<SessionStore>,
}

impl SessionRegistry {
    /// Open the on-disk store and prime the in-memory map from
    /// every persisted row. Rows with a `mode` value we don't
    /// recognize are dropped silently (forward-compat / malformed
    /// data should never crash startup).
    pub async fn open(config_base_dir: &Path) -> Result<Self, SdkError> {
        let store = SessionStore::open(config_base_dir).await?;
        let records = store.list().await?;
        let mut map: HashMap<SessionId, Arc<SessionState>> = HashMap::new();
        for rec in records {
            let Some(mode) = parse_mode(&rec.mode) else { continue };
            let id: SessionId = rec.session_id.clone().into();
            map.insert(
                id,
                Arc::new(SessionState {
                    agent: rec.agent,
                    mode,
                }),
            );
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
            store: Arc::new(store),
        })
    }

    /// Persist a new session (disk first, then memory). On disk
    /// failure the in-memory map is NOT updated — the caller sees
    /// the SDK error and the client gets a JSON-RPC init failure.
    pub async fn record(
        &self,
        id: SessionId,
        state: Arc<SessionState>,
    ) -> Result<(), SdkError> {
        self.store
            .insert(&SessionRecord {
                session_id: id.to_string(),
                agent: state.agent.clone(),
                mode: mode_str(state.mode).to_string(),
                created_at: chrono::Utc::now().timestamp(),
            })
            .await?;
        self.inner.write().await.insert(id, state);
        Ok(())
    }

    pub async fn get(&self, id: &SessionId) -> Option<Arc<SessionState>> {
        self.inner.read().await.get(id).cloned()
    }

    /// Drop from disk and memory. Best-effort: memory drop always
    /// happens; disk error is surfaced.
    pub async fn remove(&self, id: &SessionId) -> Result<(), SdkError> {
        self.inner.write().await.remove(id);
        self.store.remove(id.as_ref()).await
    }

    /// Snapshot of currently-known session ids. Used by
    /// [`crate::header_session_manager::HeaderSessionManager::new`]
    /// to rehydrate the rmcp [`LocalSessionManager`] at startup.
    pub async fn ids(&self) -> Vec<SessionId> {
        self.inner.read().await.keys().cloned().collect()
    }
}

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::Readonly => "readonly",
        Mode::Full => "full",
    }
}

/// Loose parse: accepts `"readonly"`, `"read_only"`, `"read-only"`,
/// or `"full"`, case-insensitive. Anything else returns `None`.
/// Mirrors `header_session_manager::parse_mode`.
fn parse_mode(s: &str) -> Option<Mode> {
    match s.to_ascii_lowercase().as_str() {
        "readonly" | "read_only" | "read-only" => Some(Mode::Readonly),
        "full" => Some(Mode::Full),
        _ => None,
    }
}
