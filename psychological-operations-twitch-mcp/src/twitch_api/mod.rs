//! `PsychologicalOperationsTwitchMcp` — RMCP server for Twitch.
//!
//! Mirrors `psychological-operations-discord-mcp`'s server. It exposes the read
//! tools (`whoami`, `list_channels`, `list_messages`) and the write tool
//! (`send_message`), all metered against the per-agent quota. The Twitch SDK
//! client ([`psychological_operations_sdk::twitch::Client`]) needs only the
//! `db` handle, so the server struct is slim (no reqwest / state_dir / cache /
//! mock). There's no ingest queue here yet (Twitch has no delivery), so — unlike
//! Discord — there are no queue tools.
//!
//! `tag`, `mode`, and the per-session `quota_*` overrides land here per
//! session from the `X-OBJECTIVEAI-ARGUMENTS` header — recorded by
//! [`crate::header_session_manager::HeaderSessionManager`] into
//! [`session::SessionRegistry`] keyed by `Mcp-Session-Id`.

mod model;
mod projection;
pub mod session;
mod tool_error;
pub mod tool_name;
mod tools;

use std::sync::Arc;

use psychological_operations_db::quota::DEFAULT_TOOL_COST;
use psychological_operations_db::{Db, unix_now};
use psychological_operations_sdk::twitch::Client;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::common::http_header::HEADER_SESSION_ID,
};

use crate::mode::{FULL_ONLY_TOOLS, Mode};
use session::{SessionRegistry, SessionState};
use tool_name::{Direction, ToolName};

fn is_hidden_for(mode: Mode, tool_name: &str) -> bool {
    matches!(mode, Mode::Readonly) && FULL_ONLY_TOOLS.contains(&tool_name)
}

#[derive(Clone)]
pub struct PsychologicalOperationsTwitchMcp {
    pub tool_router: ToolRouter<Self>,
    pub(super) sessions: Arc<SessionRegistry>,
    /// The single persistence layer — backs the read tools (the chat buffer +
    /// channel-join set) directly and each per-tool Twitch client.
    pub(super) db: Db,
    /// Shared Helix-backed Twitch client. Cloning is cheap. Used by the
    /// read/write tools.
    pub(super) client: Client,
}

impl std::fmt::Debug for PsychologicalOperationsTwitchMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsTwitchMcp")
            .finish_non_exhaustive()
    }
}

impl PsychologicalOperationsTwitchMcp {
    pub fn new(
        sessions: Arc<SessionRegistry>,
        db: Db,
        cache_max_size: u64,
        cache_ttl: std::time::Duration,
    ) -> Self {
        let client = Client::new(db.clone(), cache_max_size, cache_ttl);
        Self {
            tool_router: Self::read_tools() + Self::write_tools(),
            sessions,
            db,
            client,
        }
    }

    /// Resolve `Mcp-Session-Id → SessionState` for the in-flight request.
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
                ErrorData::invalid_params(format!("missing {HEADER_SESSION_ID} header"), None)
            })?;
        self.sessions
            .get(&id.to_owned().into())
            .await
            .ok_or_else(|| ErrorData::invalid_params(format!("unknown session: {id}"), None))
    }

    /// The shared Twitch client (clone is cheap — shares the inner state).
    /// Used by the read/write tools, which authenticate as the session `tag`.
    pub(super) fn build_client(&self) -> Client {
        self.client.clone()
    }

    /// Per-account, per-tool-call quota gate, run from `call_tool` before
    /// dispatch for every metered tool.
    async fn enforce_quota(
        &self,
        state: &SessionState,
        tool: ToolName,
    ) -> Result<Option<CallToolResult>, ErrorData> {
        let dir = tool.direction();
        let (used, limit) = self.quota_status(state, dir).await?;
        if used >= limit {
            let label = match dir {
                Direction::Read => "Read Quota Denial",
                Direction::Write => "Write Quota Denial",
            };
            let available = limit.saturating_sub(used);
            return Ok(Some(CallToolResult::error(vec![Content::text(format!(
                "[{label}] {used} used, {available} available"
            ))])));
        }
        self.db
            .record_tool_invocation(&state.tag, tool.as_name())
            .await
            .map_err(quota_db_err)?;
        Ok(None)
    }

    /// Windowed usage + grant-boosted limit for one direction.
    async fn quota_status(
        &self,
        state: &SessionState,
        dir: Direction,
    ) -> Result<(u64, u64), ErrorData> {
        let base = match dir {
            Direction::Read => state.quota_read,
            Direction::Write => state.quota_write,
        };
        let dir_str = match dir {
            Direction::Read => "read",
            Direction::Write => "write",
        };
        let now = unix_now();
        let cutoff = now - state.quota_interval as i64;
        let (counts, grants) = tokio::join!(
            self.db.tool_invocation_counts_since(&state.tag, cutoff),
            self.db.active_twitch_quota_grants(&state.tag, dir_str, now),
        );
        let counts = counts.map_err(quota_db_err)?;
        let grants = grants.map_err(quota_db_err)?.max(0) as u64;
        let used = sum_usage(counts, dir, &state.quota_tool_costs);
        Ok((used, base.saturating_add(grants)))
    }

    /// One-line quota summary prepended to a metered tool's result.
    async fn quota_header(&self, state: &SessionState, dir: Direction) -> Option<Content> {
        let (used, limit) = self.quota_status(state, dir).await.ok()?;
        let available = limit.saturating_sub(used);
        let label = match dir {
            Direction::Read => "Read Quota",
            Direction::Write => "Write Quota",
        };
        Some(Content::text(format!(
            "[{label}] {used} used, {available} available\n\n"
        )))
    }
}

/// Σ (count × per-tool cost) over the account's same-direction invocations.
/// Unknown tool names are ignored.
fn sum_usage(
    counts: Vec<(String, u64)>,
    dir: Direction,
    tool_costs: &std::collections::HashMap<ToolName, u64>,
) -> u64 {
    let mut usage: u64 = 0;
    for (t, n) in counts {
        let Some(tn) = ToolName::from_name(&t) else {
            continue;
        };
        if tn.direction() != dir {
            continue;
        }
        let c = tool_costs.get(&tn).copied().unwrap_or(DEFAULT_TOOL_COST);
        usage = usage.saturating_add(n.saturating_mul(c));
    }
    usage
}

/// Map a db-layer error into an rmcp `internal_error` for the quota path.
fn quota_db_err(e: psychological_operations_db::Error) -> ErrorData {
    ErrorData::internal_error(format!("quota: {e}"), None)
}

impl ServerHandler for PsychologicalOperationsTwitchMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "psychological-operations-twitch".into(),
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
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&context.extensions).await?;
        if is_hidden_for(state.mode, &request.name) {
            return Err(ErrorData::internal_error(
                format!("tool '{}' is not available in readonly mode", request.name),
                None,
            ));
        }
        // Metered tools (those in `ToolName`) are charged before they run.
        let metered = match ToolName::from_name(&request.name) {
            Some(tool) => {
                if let Some(denied) = self.enforce_quota(&state, tool).await? {
                    return Ok(denied);
                }
                Some(tool.direction())
            }
            None => None,
        };
        let tcc = ToolCallContext::new(self, request, context);
        let mut result = self.tool_router.call(tcc).await?;
        if let Some(dir) = metered {
            if let Some(header) = self.quota_header(&state, dir).await {
                result.content.insert(0, header);
            }
        }
        Ok(result)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}
