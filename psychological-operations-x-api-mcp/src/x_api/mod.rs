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

mod builders;
mod model;
mod projection;
mod tools;

use std::sync::Arc;

use psychological_operations_sdk::x::client::Client;
use psychological_operations_sdk::x::users::me as users_me;
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
};

use crate::Mode;

/// Tools only registered + callable when the server is in
/// `Mode::Full`. All mutations on X plus the queue-management
/// surface (the queue holds per-agent work, only meaningful when
/// the agent is wired for action).
const FULL_ONLY_TOOLS: &[&str] = &[
    "post_tweet",
    "reply_to_tweet",
    "quote_tweet",
    "like",
    "retweet",
    "bookmark",
    "read_queue",
    "mark_handled",
];

#[derive(Clone)]
pub struct PsychologicalOperationsXApiMcp {
    pub tool_router: ToolRouter<Self>,
    pub(super) http: Arc<Client>,
    mode: Mode,
    /// Authenticated agent name. Same string passed into
    /// `AuthMode::Agent(...)` on the inner `Client`; surfaced
    /// here so the queue tools can key reads/deletes without
    /// reaching into the Client's private auth state.
    pub(super) agent: String,
}

impl std::fmt::Debug for PsychologicalOperationsXApiMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsXApiMcp")
            .finish_non_exhaustive()
    }
}

impl PsychologicalOperationsXApiMcp {
    pub fn new(http: Arc<Client>, mode: Mode, agent: String) -> Self {
        Self {
            tool_router: tools::read_tools() + tools::write_tools() + tools::queue_tools(),
            http,
            mode,
            agent,
        }
    }

    /// `true` when this tool is registered but should not be listed
    /// or callable in the current mode.
    fn is_hidden(&self, tool_name: &str) -> bool {
        matches!(self.mode, Mode::Readonly) && FULL_ONLY_TOOLS.contains(&tool_name)
    }

    /// Resolve the authenticated user's numeric id via `/users/me`.
    /// Used by the engagement tools (like / retweet / bookmark)
    /// that need the acting user id in the URL path.
    pub(super) async fn resolve_self_user_id(&self) -> Result<String, ErrorData> {
        let req = users_me::get::Request {
            user_fields: None,
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_me::http::get(&self.http, &req)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/me: {e}"), None))?;
        let user = resp.data.ok_or_else(|| {
            ErrorData::internal_error("users/me had no data".to_string(), None)
        })?;
        Ok(user.id.0)
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
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools: Vec<Tool> = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| !self.is_hidden(&t.name))
            .collect();
        Ok(ListToolsResult { tools, next_cursor: None })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if self.is_hidden(&request.name) {
            return Err(ErrorData::invalid_params(
                format!("tool '{}' is not available in readonly mode", request.name),
                None,
            ));
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if self.is_hidden(name) {
            None
        } else {
            self.tool_router.get(name).cloned()
        }
    }
}
