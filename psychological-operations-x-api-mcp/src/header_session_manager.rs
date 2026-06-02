//! `SessionManager` wrapper that reads the two psyop-specific
//! headers off the **initial** initialize request and stashes them
//! on a disk-backed [`SessionRegistry`], then delegates the rest of
//! the rmcp session lifecycle to [`LocalSessionManager`].
//!
//! On reconnect the client may send the same `Mcp-Session-Id` it
//! got on its first connect; tower's reconnect branch
//! (`rmcp-0.16.0/src/transport/streamable_http_server/tower.rs:304-372`)
//! never calls `initialize_session`, so we never re-read the
//! headers — the persisted `(agent, mode)` is the source of truth.
//! If the persisted state is missing (e.g. stale id from a wiped
//! sessions db), `has_session(id)` returns false and tower
//! responds with `401 Unauthorized: Session not found`.

use std::sync::Arc;

use futures::Stream;
use rmcp::model::{
    ClientCapabilities, ClientJsonRpcMessage, ClientRequest, GetExtensions, Implementation,
    InitializeRequestParams, JsonRpcRequest, JsonRpcVersion2_0, NumberOrString, ProtocolVersion,
    Request, ServerJsonRpcMessage,
};
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionHandle, LocalSessionManager, LocalSessionManagerError, SessionError,
};
use rmcp::transport::streamable_http_server::session::{ServerSseMessage, SessionId};

use crate::Mode;
use crate::x_api::session::{HEADER_AGENT, HEADER_MODE, SessionRegistry, SessionState};

#[derive(Debug, Clone)]
pub struct HeaderSessionManager {
    inner: Arc<LocalSessionManager>,
    registry: Arc<SessionRegistry>,
}

impl HeaderSessionManager {
    pub fn new(registry: Arc<SessionRegistry>) -> Self {
        Self {
            inner: Arc::new(LocalSessionManager::default()),
            registry,
        }
    }

    /// The wrapped rmcp `LocalSessionManager`. Exposed so
    /// `run.rs::setup` can directly insert resurrected
    /// [`LocalSessionHandle`]s when restoring persisted sessions
    /// at startup (rmcp doesn't expose a public method on
    /// `SessionManager` for "register an externally-built
    /// handle"; the `sessions` HashMap inside `LocalSessionManager`
    /// is itself `pub`).
    pub fn inner(&self) -> &Arc<LocalSessionManager> {
        &self.inner
    }

    pub fn registry(&self) -> &Arc<SessionRegistry> {
        &self.registry
    }
}

impl SessionManager for HeaderSessionManager {
    type Error = LocalSessionManagerError;
    type Transport = <LocalSessionManager as SessionManager>::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        self.inner.create_session().await
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        // Tower only routes here when the client connected WITHOUT
        // Mcp-Session-Id (fresh session). Extract the two headers,
        // record into the disk-backed registry, then delegate.
        let state = extract_session_state(&message).map_err(error_invalid_input)?;
        self.registry
            .record(id.clone(), Arc::new(state))
            .await
            .map_err(error_io_other)?;
        self.inner.initialize_session(id, message).await
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.inner.has_session(id).await
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        let _ = self.registry.remove(id).await;
        self.inner.close_session(id).await
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner.create_stream(id, message).await
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.inner.accept_message(id, message).await
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner.create_standalone_stream(id).await
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner.resume(id, last_event_id).await
    }
}

/// Build a minimal-but-valid `initialize` JSON-RPC request used
/// during startup rehydration to drive a freshly-spawned worker
/// past its initial `SessionEvent::InitializeRequest` wait state
/// (`local.rs:858-870`). No real client receives the response —
/// the worker is just being seeded so a later real-client
/// reconnect can run through it as a normal `ClientMessage`.
pub fn synthetic_initialize_message() -> ClientJsonRpcMessage {
    let request = Request {
        method: Default::default(),
        params: InitializeRequestParams {
            meta: None,
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "psychological-operations-x-api-mcp-restore-stub".into(),
                title: None,
                version: "0".into(),
                description: None,
                icons: None,
                website_url: None,
            },
        },
        extensions: Default::default(),
    };
    ClientJsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JsonRpcVersion2_0,
        id: NumberOrString::Number(0),
        request: ClientRequest::InitializeRequest(request),
    })
}

fn extract_session_state(message: &ClientJsonRpcMessage) -> Result<SessionState, String> {
    let parts = match message {
        ClientJsonRpcMessage::Request(req) => {
            req.request.extensions().get::<http::request::Parts>()
        }
        _ => None,
    }
    .ok_or_else(|| "initialize request missing injected HTTP parts extension".to_string())?;

    let agent = parts
        .headers
        .get(HEADER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or empty {HEADER_AGENT} header"))?;

    let mode_str = parts
        .headers
        .get(HEADER_MODE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or empty {HEADER_MODE} header"))?;

    let mode = parse_mode(&mode_str)
        .ok_or_else(|| format!("{HEADER_MODE}: expected 'readonly' or 'full', got {mode_str:?}"))?;

    Ok(SessionState { agent, mode })
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s.to_ascii_lowercase().as_str() {
        "readonly" | "read_only" | "read-only" => Some(Mode::Readonly),
        "full" => Some(Mode::Full),
        _ => None,
    }
}

fn error_invalid_input(msg: String) -> LocalSessionManagerError {
    LocalSessionManagerError::SessionError(SessionError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        msg,
    )))
}

fn error_io_other<E: std::fmt::Display>(e: E) -> LocalSessionManagerError {
    LocalSessionManagerError::SessionError(SessionError::Io(std::io::Error::other(e.to_string())))
}
