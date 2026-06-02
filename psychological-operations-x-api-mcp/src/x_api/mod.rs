//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace SDK (`psychological_operations_sdk::x::*`).
//!
//! Every tool drives the codegen'd `Request`/`Response` types directly
//! via the codegen'd per-endpoint `http` helpers (which already know
//! the URL template, encoding, and the `send_with_query` call). The
//! only custom tweet struct anywhere in this codebase lives in
//! [`model::Tweet`] — the small, agent-facing projection that drops
//! the ~30 optional fields the X spec carries on its Tweet schema
//! and keeps the ones the agent actually consumes (id, handle,
//! content, attachments, plus the three optional reference IDs
//! replied_to / quoted / retweeted).
//!
//! Binary media bytes come from `Client::fetch_url` — the SDK's sole
//! hand-written non-codegen call (twimg has no OpenAPI surface).
//!
//! `agent` and `mode` are NOT process-wide. They land here per
//! session via the `X-PSYOP-X-API-AGENT` / `X-PSYOP-X-API-MODE`
//! headers on the initialize request — recorded by
//! [`crate::header_session_manager::HeaderSessionManager`] into
//! [`session::SessionRegistry`] keyed by `Mcp-Session-Id`. Tool
//! handlers look the pair up via [`PsychologicalOperationsXApiMcp::resolve_session`]
//! and build a fresh SDK [`Client`] for each call via
//! [`PsychologicalOperationsXApiMcp::build_client`].

mod builders;
mod model;
mod projection;
pub mod session;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use psychological_operations_sdk::x::client::{AuthMode, Client};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResult, Implementation,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::common::http_header::HEADER_SESSION_ID,
};

use crate::Mode;
use session::{SessionRegistry, SessionState};

/// Tools only listed + callable when the *session's* mode is
/// `Mode::Full`. Read tools (and `whoami`) are always exposed.
const FULL_ONLY_TOOLS: &[&str] = &[
    "post",
    "reply",
    "quote",
    "like",
    "retweet",
    "bookmark",
    "read_queue",
    "mark_handled",
];

fn is_hidden_for(mode: Mode, tool_name: &str) -> bool {
    matches!(mode, Mode::Readonly) && FULL_ONLY_TOOLS.contains(&tool_name)
}

#[derive(Clone)]
pub struct PsychologicalOperationsXApiMcp {
    pub tool_router: ToolRouter<Self>,
    pub(super) sessions: Arc<SessionRegistry>,
    /// Shared HTTP connection pool. Cloning a `reqwest::Client` is
    /// cheap (Arc internally), so every per-tool SDK `Client` we
    /// build reuses this pool.
    pub(super) reqwest: reqwest::Client,
    pub(super) config_base_dir: PathBuf,
    pub(super) cache_max_size: u64,
    pub(super) cache_ttl: Duration,
}

impl std::fmt::Debug for PsychologicalOperationsXApiMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsXApiMcp")
            .finish_non_exhaustive()
    }
}

impl PsychologicalOperationsXApiMcp {
    pub fn new(
        sessions: Arc<SessionRegistry>,
        reqwest: reqwest::Client,
        config_base_dir: PathBuf,
        cache_max_size: u64,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            tool_router: tools::read_tools() + tools::write_tools() + tools::queue_tools(),
            sessions,
            reqwest,
            config_base_dir,
            cache_max_size,
            cache_ttl,
        }
    }

    /// Resolve `Mcp-Session-Id → SessionState` for the currently
    /// in-flight request. Returns an `invalid_params` rmcp error if
    /// the request didn't carry a session id, or if the id doesn't
    /// match any session we've initialized.
    pub(super) async fn resolve_session(
        &self,
        extensions: &rmcp::model::Extensions,
    ) -> Result<Arc<SessionState>, ErrorData> {
        let parts = extensions.get::<http::request::Parts>().ok_or_else(|| {
            ErrorData::internal_error(
                "missing http request parts on rmcp request".to_string(),
                None,
            )
        })?;
        let id = parts
            .headers
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("missing {HEADER_SESSION_ID} header"),
                    None,
                )
            })?;
        self.sessions
            .get(&id.to_owned().into())
            .await
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("unknown session: {id}"), None)
            })
    }

    /// Build a fresh SDK [`Client`] bound to the given `agent`,
    /// reusing the process-wide reqwest pool and the current
    /// process-wide cache config. Called once per tool invocation —
    /// `Client::new` is infallible + synchronous and the SQLite
    /// `OnceCell`s lazy-init on first use.
    pub(super) fn build_client(&self, agent: &str) -> Client {
        Client::new(
            self.reqwest.clone(),
            /* mock */ false,
            self.cache_max_size,
            self.cache_ttl,
            self.config_base_dir.clone(),
            AuthMode::Agent(agent.to_string()),
        )
    }
}

impl ServerHandler for PsychologicalOperationsXApiMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "psychological-operations-x-api".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Pre-initialize `list_tools` isn't possible by the MCP
        // spec (the client must initialize first), so the session
        // should always be present. If it somehow isn't, fall back
        // to the safe (readonly) surface instead of erroring — the
        // client can still see what's available.
        let mode = match self.resolve_session(&context.extensions).await {
            Ok(state) => state.mode,
            Err(_) => Mode::Readonly,
        };
        let tools: Vec<Tool> = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| !is_hidden_for(mode, &t.name))
            .collect();
        Ok(ListToolsResult { tools, next_cursor: None })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&context.extensions).await?;
        if is_hidden_for(state.mode, &request.name) {
            return Err(ErrorData::invalid_params(
                format!(
                    "tool '{}' is not available in readonly mode",
                    request.name
                ),
                None,
            ));
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        // No session context here — return the tool definition if
        // we have it. Mode-gating happens in `list_tools` /
        // `call_tool` where we DO have context.
        self.tool_router.get(name).cloned()
    }
}
