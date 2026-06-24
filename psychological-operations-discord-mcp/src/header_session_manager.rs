//! Custom `SessionManager`. Two non-default behaviors that together make
//! session-id-as-identity disappear:
//!
//! 1. **`has_session` always returns `Ok(true)`.** Tower never 401s.
//! 2. **Lazy `(handle, worker)` mint on first POST.** When tower routes a
//!    request for an id the inner `LocalSessionManager` doesn't hold, we pull
//!    the session args (tag/mode/quota) off the current message's injected
//!    `http::request::Parts` (per [`crate::discord_api::session::HEADER_ARGUMENTS`]),
//!    register `SessionState`, spawn the worker + service end, and drive the
//!    worker past its initial `InitializeRequest` wait with a synthetic stub.
//!
//! Net effect: objectiveai keeps re-sending the per-URL
//! `X-OBJECTIVEAI-ARGUMENTS` on every connect; the server keeps state in
//! memory only; a process restart silently rebuilds the session entry on the
//! next request. No disk.
//!
//! This is a near-verbatim copy of the X MCP's manager (only the server type
//! differs); see that crate for the deeper commentary.

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
    LocalSessionHandle, LocalSessionManager, LocalSessionManagerError, SessionConfig, SessionError,
    create_local_session,
};
use rmcp::transport::streamable_http_server::session::{ServerSseMessage, SessionId};

use psychological_operations_db::quota::{
    DEFAULT_INTERVAL_SECS, DEFAULT_READ_LIMIT, DEFAULT_TOOL_COST, DEFAULT_WRITE_LIMIT,
};

use crate::Mode;
use crate::PsychologicalOperationsDiscordMcp;
use crate::ToolName;
use crate::discord_api::session::{
    DEFAULT_MAX_MESSAGE_LENGTH, HEADER_ARGUMENTS, SessionRegistry, SessionState,
};

#[derive(Debug, Clone)]
pub struct HeaderSessionManager {
    inner: Arc<LocalSessionManager>,
    registry: Arc<SessionRegistry>,
    /// Used by `ensure_session` to spawn a service end onto each lazy-created
    /// worker.
    service: PsychologicalOperationsDiscordMcp,
}

impl HeaderSessionManager {
    pub fn new(
        registry: Arc<SessionRegistry>,
        service: PsychologicalOperationsDiscordMcp,
    ) -> Self {
        Self {
            inner: Arc::new(LocalSessionManager::default()),
            registry,
            service,
        }
    }

    async fn mint_worker(
        &self,
        id: &SessionId,
        message: &ClientJsonRpcMessage,
    ) -> Result<LocalSessionHandle, LocalSessionManagerError> {
        let state = extract_session_state(message).map_err(error_invalid_input)?;
        self.registry.record(id.clone(), Arc::new(state)).await;

        let (handle, worker) = create_local_session(id.clone(), SessionConfig::default());
        let transport = WorkerTransport::spawn(worker);

        let svc = self.service.clone();
        let id_for_close = id.clone();
        let registry_for_close = self.registry.clone();
        let inner_for_close = self.inner.clone();
        tokio::spawn(async move {
            let res = serve_server::<_, _, _, TransportAdapterIdentity>(svc, transport).await;
            if let Ok(svc) = res {
                let _ = svc.waiting().await;
            }
            let _ = registry_for_close.remove(&id_for_close).await;
            inner_for_close.sessions.write().await.remove(&id_for_close);
        });

        Ok(handle)
    }

    async fn ensure_session(
        &self,
        id: &SessionId,
        message: &ClientJsonRpcMessage,
    ) -> Result<(), LocalSessionManagerError> {
        if self.inner.has_session(id).await? {
            return Ok(());
        }
        let handle = self.mint_worker(id, message).await?;
        handle
            .initialize(synthetic_initialize_message())
            .await
            .map_err(|e| error_invalid_input(format!("synthetic initialize: {e}")))?;
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
        let state = extract_session_state(&message).map_err(error_invalid_input)?;
        self.registry.record(id.clone(), Arc::new(state)).await;
        self.inner.initialize_session(id, message).await
    }

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
        if is_initialize(&message) && !self.inner.has_session(id).await? {
            let handle = self.mint_worker(id, &message).await?;
            let response = handle
                .initialize(message)
                .await
                .map_err(|e| error_invalid_input(format!("resume initialize: {e}")))?;
            self.inner.sessions.write().await.insert(id.clone(), handle);
            let item = ServerSseMessage {
                event_id: None,
                message: Some(Arc::new(response)),
                retry: None,
            };
            let stream: std::pin::Pin<
                Box<dyn Stream<Item = ServerSseMessage> + Send + Sync + 'static>,
            > = Box::pin(futures::stream::iter(vec![item]));
            return Ok(stream);
        }
        self.ensure_session(id, &message).await?;
        let inner = self.inner.create_stream(id, message).await?;
        let stream: std::pin::Pin<Box<dyn Stream<Item = ServerSseMessage> + Send + Sync + 'static>> =
            Box::pin(inner);
        Ok(stream)
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

fn is_initialize(m: &ClientJsonRpcMessage) -> bool {
    matches!(
        m,
        ClientJsonRpcMessage::Request(r)
            if matches!(r.request, ClientRequest::InitializeRequest(_))
    )
}

pub fn synthetic_initialize_message() -> ClientJsonRpcMessage {
    let request = Request {
        method: Default::default(),
        params: InitializeRequestParams {
            meta: None,
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "psychological-operations-discord-mcp-restore-stub".into(),
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
        ClientJsonRpcMessage::Notification(not) => {
            not.notification.extensions().get::<http::request::Parts>()
        }
        _ => None,
    }
    .ok_or_else(|| "message missing injected HTTP parts extension".to_string())?;

    let args: serde_json::Map<String, serde_json::Value> = parts
        .headers
        .get(HEADER_ARGUMENTS)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let mode_str = lookup_string_ci(&args, "mode")
        .ok_or_else(|| format!("missing mode: {HEADER_ARGUMENTS}[\"mode\"] absent or empty"))?;
    let mode = parse_mode(&mode_str)
        .ok_or_else(|| format!("mode: expected 'readonly' or 'full', got {mode_str:?}"))?;

    let tag = lookup_string_ci(&args, "tag")
        .ok_or_else(|| format!("missing tag: {HEADER_ARGUMENTS}[\"tag\"] absent or empty"))?;

    let max_message_length =
        parse_u64_arg(&args, "max_message_length")?.unwrap_or(DEFAULT_MAX_MESSAGE_LENGTH) as usize;

    let quota_read = parse_u64_arg(&args, "quota_read")?.unwrap_or(DEFAULT_READ_LIMIT);
    let quota_write = parse_u64_arg(&args, "quota_write")?.unwrap_or(DEFAULT_WRITE_LIMIT);
    let quota_interval =
        parse_interval_secs_arg(&args, "quota_interval")?.unwrap_or(DEFAULT_INTERVAL_SECS);
    // No metered tools yet → `ToolName::ALL` is empty → no `quota_usage_<tool>`
    // entries. `DEFAULT_TOOL_COST` referenced to keep parity with X once tools
    // (and their per-tool cost args) are added.
    let quota_tool_costs = ToolName::ALL
        .iter()
        .map(|&t| {
            let key = format!("quota_usage_{}", t.as_name());
            let cost = parse_u64_arg(&args, &key)?.unwrap_or(DEFAULT_TOOL_COST);
            Ok((t, cost))
        })
        .collect::<Result<std::collections::HashMap<ToolName, u64>, String>>()?;

    Ok(SessionState {
        tag,
        mode,
        max_message_length,
        quota_read,
        quota_write,
        quota_interval,
        quota_tool_costs,
    })
}

fn lookup_string_ci(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_u64_arg(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, String> {
    let Some((_, v)) = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) else {
        return Ok(None);
    };
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .map(Some)
        .ok_or_else(|| {
            format!("{HEADER_ARGUMENTS}[{key:?}]: expected a non-negative integer, got {v}")
        })
}

fn parse_interval_secs_arg(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, String> {
    let Some((_, v)) = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) else {
        return Ok(None);
    };
    let s = v.as_str().ok_or_else(|| {
        format!("{HEADER_ARGUMENTS}[{key:?}]: expected a humantime duration string, got {v}")
    })?;
    let dur = humantime::parse_duration(s.trim()).map_err(|e| {
        format!(
            "{HEADER_ARGUMENTS}[{key:?}]: expected a humantime duration (e.g. '1h'), got {s:?}: {e}"
        )
    })?;
    Ok(Some(dur.as_secs()))
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "readonly" => Some(Mode::Readonly),
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
