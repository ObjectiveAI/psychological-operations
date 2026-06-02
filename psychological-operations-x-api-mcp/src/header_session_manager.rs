//! `SessionManager` wrapper that reads the two psyop-specific
//! headers off the initialize request and stashes them on
//! [`SessionRegistry`] keyed by session id, then delegates the rest
//! of the lifecycle to rmcp's [`LocalSessionManager`].
//!
//! Why a wrapper instead of replacing the manager outright:
//! [`LocalSessionManager`] already knows how to drive the in-process
//! session worker (mpsc transport, message routing, Last-Event-ID
//! replay). We only need to inject behavior at the two boundaries
//! where the per-session `(agent, mode)` enters and leaves —
//! `initialize_session` and `close_session`. Everything else is a
//! pass-through.

use std::sync::Arc;

use futures::Stream;
use rmcp::model::{ClientJsonRpcMessage, GetExtensions, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError,
};
use rmcp::transport::streamable_http_server::session::{ServerSseMessage, SessionId};

use crate::Mode;
use crate::x_api::session::{HEADER_AGENT, HEADER_MODE, SessionRegistry, SessionState};

#[derive(Debug, Default)]
pub struct HeaderSessionManager {
    inner: LocalSessionManager,
    registry: Arc<SessionRegistry>,
}

impl HeaderSessionManager {
    pub fn new(registry: Arc<SessionRegistry>) -> Self {
        Self {
            inner: LocalSessionManager::default(),
            registry,
        }
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
        // Extract the two headers off the initialize request's
        // injected `http::request::Parts`. If either is missing or
        // malformed, the session is not viable — return an error so
        // the streamable-HTTP layer surfaces it as a JSON-RPC error
        // and the client can fix its headers and retry.
        let state = extract_session_state(&message)
            .map_err(|e| LocalSessionManagerError::SessionError(
                rmcp::transport::streamable_http_server::session::local::SessionError::Io(
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
                ),
            ))?;
        self.registry.record(id.clone(), Arc::new(state)).await;
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

fn extract_session_state(message: &ClientJsonRpcMessage) -> Result<SessionState, String> {
    let parts = match message {
        ClientJsonRpcMessage::Request(req) => req.request.extensions().get::<http::request::Parts>(),
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
