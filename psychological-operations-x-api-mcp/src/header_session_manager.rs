//! Custom `SessionManager`. Two non-default behaviors that together
//! make session-id-as-identity disappear:
//!
//! 1. **`has_session` always returns `Ok(true)`.** Tower never
//!    401s. Any session id the client presents is treated as
//!    valid for routing purposes.
//! 2. **Lazy `(handle, worker)` mint on first POST.** When tower
//!    routes a request through `create_stream` or `accept_message`
//!    for an id the inner `LocalSessionManager` doesn't currently
//!    hold, we pull `(agent, mode)` off the current message's
//!    injected `http::request::Parts` (per the source resolution
//!    documented on
//!    [`crate::x_api::session::HEADER_ARGUMENTS`]), register
//!    `SessionState`, spawn the worker + service end, and drive
//!    the worker past its initial `SessionEvent::InitializeRequest`
//!    wait state with a synthetic stub. The original message then
//!    delegates to the inner manager and rides through as if the
//!    session had existed all along.
//!
//! The net effect: objectiveai keeps re-sending the per-URL
//! `X-OBJECTIVEAI-ARGUMENTS` (+ session-global
//! `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY`) on every connect;
//! the server keeps state in memory only; a process restart
//! silently rebuilds the (agent, mode) entry on the very next
//! request. No disk.

use std::sync::Arc;

use futures::Stream;
use rmcp::model::{
    ClientCapabilities, ClientJsonRpcMessage, ClientRequest, GetExtensions, Implementation,
    InitializeRequestParams, JsonRpcRequest, JsonRpcVersion2_0, NumberOrString, ProtocolVersion,
    Request, ServerJsonRpcMessage,
};
use rmcp::service::serve_server;
use rmcp::transport::TransportAdapterIdentity;
use rmcp::transport::WorkerTransport;
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError, SessionConfig, SessionError,
    create_local_session,
};
use rmcp::transport::streamable_http_server::session::{ServerSseMessage, SessionId};

use crate::Mode;
use crate::PsychologicalOperationsXApiMcp;
use crate::x_api::session::{
    HEADER_AGENT_INSTANCE_HIERARCHY, HEADER_ARGUMENTS, SessionRegistry, SessionState,
};

#[derive(Debug, Clone)]
pub struct HeaderSessionManager {
    inner: Arc<LocalSessionManager>,
    registry: Arc<SessionRegistry>,
    /// Used by `ensure_session` to spawn a service end onto each
    /// lazy-created worker.
    service: PsychologicalOperationsXApiMcp,
    /// Launch-flag fallbacks for `agent` / `mode` (from
    /// `mcp x-api begin --agent <a> --mode <m>`). objectiveai's conduit
    /// spawns one server per (plugin, args) and passes the agent/mode as
    /// launch flags; in some host versions those don't also ride the
    /// per-request `X-OBJECTIVEAI-ARGUMENTS` header. When the header
    /// lacks them, these stand in — so a conduit-launched server still
    /// resolves its session. `None` ⇒ no fallback (header is the only
    /// source, as before).
    default_agent: Option<String>,
    default_mode: Option<Mode>,
}

impl HeaderSessionManager {
    pub fn new(
        registry: Arc<SessionRegistry>,
        service: PsychologicalOperationsXApiMcp,
        default_agent: Option<String>,
        default_mode: Option<Mode>,
    ) -> Self {
        Self {
            inner: Arc::new(LocalSessionManager::default()),
            registry,
            service,
            default_agent,
            default_mode,
        }
    }

    /// Make sure the inner `LocalSessionManager` has a handle for
    /// `id`. If it already does, no-op. Otherwise extract the
    /// X-OBJECTIVEAI-* headers from the current message, register
    /// `SessionState`, mint a worker, attach a service, and feed
    /// a synthetic initialize so the worker is ready to receive
    /// the real client message in its main loop.
    async fn ensure_session(
        &self,
        id: &SessionId,
        message: &ClientJsonRpcMessage,
    ) -> Result<(), LocalSessionManagerError> {
        if self.inner.has_session(id).await? {
            return Ok(());
        }

        let state = extract_session_state(
            message,
            self.default_agent.as_deref(),
            self.default_mode,
        )
        .map_err(error_invalid_input)?;
        self.registry.record(id.clone(), Arc::new(state)).await;

        let (handle, worker) = create_local_session(id.clone(), SessionConfig::default());
        let transport = WorkerTransport::spawn(worker);

        // Service-side task. Cleanup mirrors the pattern at
        // `rmcp-0.16.0/src/transport/streamable_http_server/tower.rs:392-416`
        // — when the service ends (worker died, transport closed)
        // we drop the entry from both maps.
        let svc = self.service.clone();
        let id_for_close = id.clone();
        let registry_for_close = self.registry.clone();
        let inner_for_close = self.inner.clone();
        tokio::spawn(async move {
            let res =
                serve_server::<_, _, _, TransportAdapterIdentity>(svc, transport).await;
            if let Ok(svc) = res {
                let _ = svc.waiting().await;
            }
            let _ = registry_for_close.remove(&id_for_close).await;
            inner_for_close.sessions.write().await.remove(&id_for_close);
        });

        // Drive the worker past its initial
        // `SessionEvent::InitializeRequest` wait state
        // (`local.rs:858-870`). The response is discarded; the
        // real client (if its current message is itself an
        // initialize) will overwrite peer_info on the next pass
        // through the worker's main loop.
        handle
            .initialize(synthetic_initialize_message())
            .await
            .map_err(|e| {
                error_invalid_input(format!("synthetic initialize: {e}"))
            })?;

        self.inner.sessions.write().await.insert(id.clone(), handle);
        Ok(())
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
        // No-session-id POST path: extract headers, record state,
        // delegate. The inner already has the handle from its
        // own `create_session` (called by tower right before
        // this).
        let state = extract_session_state(
            &message,
            self.default_agent.as_deref(),
            self.default_mode,
        )
        .map_err(error_invalid_input)?;
        self.registry.record(id.clone(), Arc::new(state)).await;
        self.inner.initialize_session(id, message).await
    }

    /// Always `Ok(true)`. Tower's reject-with-401 path never fires
    /// for us; the validity of a session id is established
    /// lazily by `ensure_session` reading headers off the very
    /// request that uses it.
    async fn has_session(&self, _id: &SessionId) -> Result<bool, Self::Error> {
        Ok(true)
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
        self.ensure_session(id, &message).await?;
        self.inner.create_stream(id, message).await
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.ensure_session(id, &message).await?;
        self.inner.accept_message(id, message).await
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        // GET path: no message, no headers we can extract here.
        // If the inner doesn't already know the session, the
        // client gets rmcp's standard "session not found" from
        // this path. The CLI's MCP client uses POST.
        self.inner.create_standalone_stream(id).await
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        // Same GET-path constraint as `create_standalone_stream`.
        self.inner.resume(id, last_event_id).await
    }
}

/// Minimal-but-valid `initialize` JSON-RPC request used during
/// lazy `ensure_session` rehydration. Drives the freshly-spawned
/// worker past its initial `SessionEvent::InitializeRequest` wait
/// state. `ServerHandler::initialize`'s default impl is
/// idempotent (set_peer_info overwrites on the next call), so the
/// real client's subsequent initialize — if any — wins.
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

fn extract_session_state(
    message: &ClientJsonRpcMessage,
    default_agent: Option<&str>,
    default_mode: Option<Mode>,
) -> Result<SessionState, String> {
    let parts = match message {
        ClientJsonRpcMessage::Request(req) => {
            req.request.extensions().get::<http::request::Parts>()
        }
        ClientJsonRpcMessage::Notification(not) => {
            not.notification.extensions().get::<http::request::Parts>()
        }
        _ => None,
    }
    .ok_or_else(|| "message missing injected HTTP parts extension".to_string())?;

    // Parse X-OBJECTIVEAI-ARGUMENTS as a JSON object. Absent /
    // malformed / non-object → empty map. Per-key fallbacks below
    // still apply.
    let args: serde_json::Map<String, serde_json::Value> = parts
        .headers
        .get(HEADER_ARGUMENTS)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // The session-global agent-id chain. Doubles as the fallback
    // source for `agent` below, and as the per-caller key for the
    // API request log (quota ledger).
    let hierarchy = parts
        .headers
        .get(HEADER_AGENT_INSTANCE_HIERARCHY)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // `agent`: try the JSON args first (case-insensitive key), then the
    // X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY header, then the launch-flag
    // fallback (`begin --agent`). Error only if every source is empty.
    let agent = lookup_string_ci(&args, "agent")
        .or_else(|| hierarchy.clone())
        .or_else(|| {
            default_agent
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| format!(
            "missing agent: {HEADER_ARGUMENTS}[\"agent\"] absent or empty, \
             {HEADER_AGENT_INSTANCE_HIERARCHY} header absent or empty, \
             and no --agent launch fallback"
        ))?;

    // `mode`: JSON args first, then the launch-flag fallback
    // (`begin --mode`), then `Mode::Readonly`. A malformed header value
    // is still an error.
    let mode = match lookup_string_ci(&args, "mode") {
        Some(s) => parse_mode(&s).ok_or_else(|| {
            format!("mode: expected 'readonly' or 'full', got {s:?}")
        })?,
        None => default_mode.unwrap_or(Mode::Readonly),
    };

    // Header absent ⇒ the resolved agent (which, in that case,
    // necessarily came from the args map) stands in as the
    // ledger key.
    let agent_instance_hierarchy = hierarchy.unwrap_or_else(|| agent.clone());

    Ok(SessionState { agent, mode, agent_instance_hierarchy })
}

/// Case-insensitive key lookup over a JSON object. Returns the
/// matched value as a trimmed non-empty `String`, or `None` if no
/// key matches, the matched value isn't a string, or it trims to
/// empty.
fn lookup_string_ci(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
