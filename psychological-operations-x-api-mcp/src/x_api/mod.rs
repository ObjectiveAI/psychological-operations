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
//! session, sourced from the `X-OBJECTIVEAI-ARGUMENTS` JSON-object
//! header (with `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the
//! agent fallback) on every request — recorded by
//! [`crate::header_session_manager::HeaderSessionManager`] into
//! [`session::SessionRegistry`] keyed by `Mcp-Session-Id`. Tool
//! handlers look the pair up via [`PsychologicalOperationsXApiMcp::resolve_session`]
//! and build a fresh SDK [`Client`] for each call via
//! [`PsychologicalOperationsXApiMcp::build_client`].

pub mod accounts;
mod builders;
mod model;
mod projection;
pub mod session;
mod tool_error;
pub mod tool_name;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use psychological_operations_db::quota::DEFAULT_TOOL_COST;
use psychological_operations_db::{Db, QuotaDirection, unix_now};
use psychological_operations_sdk::x::client::Client;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResult, Content, Implementation,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::common::http_header::HEADER_SESSION_ID,
};

use crate::Mode;
use accounts::AgentTagLister;
use session::{SessionRegistry, SessionState};
use tool_name::{Direction, ToolName};

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
    /// Root of all on-disk state (the `OBJECTIVEAI_STATE_DIR` value);
    /// passed straight to each per-tool SDK `Client` (CEF cookie probe).
    pub(super) state_dir: PathBuf,
    /// The single persistence layer, cloned into each per-tool Client.
    pub(super) db: Db,
    pub(super) cache_max_size: u64,
    pub(super) cache_ttl: Duration,
    /// When true, every per-tool SDK `Client` is built in mock mode —
    /// X-API calls short-circuit to the in-process deterministic mock
    /// instead of hitting the real network. Sourced from
    /// `PSYCHOLOGICAL_OPERATIONS_MOCK` (standalone binary) or the CLI's
    /// `Config.mock` (`mcp begin`).
    pub(super) mock: bool,
    /// Discovers an agent's usable identities (its AIH plus its tags)
    /// for `list_accounts`, wrapping the objectiveai `CommandExecutor`
    /// behind an object-safe trait (the executor itself is generic and
    /// not object-safe). One operation: `agents instances get`.
    pub(super) accounts: Arc<dyn AgentTagLister>,
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
        state_dir: PathBuf,
        db: Db,
        cache_max_size: u64,
        cache_ttl: Duration,
        mock: bool,
        accounts: Arc<dyn AgentTagLister>,
    ) -> Self {
        Self {
            tool_router: Self::read_tools()
                + Self::write_tools()
                + Self::queue_tools()
                + Self::accounts_tools(),
            sessions,
            reqwest,
            state_dir,
            db,
            cache_max_size,
            cache_ttl,
            mock,
            accounts,
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

    /// Build a fresh SDK [`Client`], reusing the process-wide reqwest
    /// pool, cache config, state dir, and `db` handle. Called once per
    /// tool invocation — `Client::new` is infallible + synchronous. The
    /// identity to act as is no longer baked in here: each tool builds
    /// `AuthMode::Agent(req.account)` from its required `account` arg
    /// and passes it to the generated X-API calls.
    pub(super) fn build_client(&self) -> Client {
        Client::new(
            self.reqwest.clone(),
            self.mock,
            self.cache_max_size,
            self.cache_ttl,
            self.state_dir.clone(),
            self.db.clone(),
        )
    }

    /// Per-account, per-tool-call quota gate, run from `call_tool`
    /// **before** dispatch for every metered tool. Quota is MCP-specific
    /// (it ignores the SDK cache + real X-API entirely): each tool is
    /// intrinsically a read XOR a write with a configurable cost, charged
    /// against the acting `account`'s read/write budget over a trailing
    /// per-direction window. On success the invocation is appended to the
    /// bare ledger; on overflow an `invalid_params` error is returned and
    /// nothing is recorded.
    /// Returns `Ok(None)` to proceed, or `Ok(Some(result))` carrying a
    /// quota-denial [`CallToolResult`] (`is_error`) to surface back to the
    /// agent as ordinary tool output rather than a JSON-RPC protocol
    /// error — so the model reads "you're out of quota" and can react,
    /// instead of the call hard-failing at the transport. `Err` is
    /// reserved for genuine db failures.
    async fn enforce_quota(
        &self,
        account: &str,
        tool: ToolName,
    ) -> Result<Option<CallToolResult>, ErrorData> {
        let dir = tool.direction();
        let qdir = direction_to_quota(dir);
        let cfg = self.db.quota_config(account).await.map_err(quota_db_err)?;
        let (limit, interval) = cfg.for_direction(qdir);
        // Deny only when usage is already at/above the limit. The call's
        // own cost doesn't gate it — a single call may push usage past
        // the limit (which then blocks the *next* same-direction call),
        // so an expensive tool stays usable as long as there's any
        // headroom left.
        let usage = self.quota_used(account, dir, interval).await?;
        if usage >= limit {
            let label = match dir {
                Direction::Read => "Read Quota Denial",
                Direction::Write => "Write Quota Denial",
            };
            let available = limit.saturating_sub(usage);
            return Ok(Some(CallToolResult::error(vec![Content::text(format!(
                "[{label}] {usage} used, {available} available"
            ))])));
        }
        self.db
            .record_tool_invocation(account, tool.as_name())
            .await
            .map_err(quota_db_err)?;
        Ok(None)
    }

    /// Windowed usage for one direction: Σ (count × per-tool cost) over
    /// the account's same-direction invocations newer than
    /// `now - interval_secs`. Unknown tool names (drift / renamed tools)
    /// are ignored.
    async fn quota_used(
        &self,
        account: &str,
        dir: Direction,
        interval_secs: u64,
    ) -> Result<u64, ErrorData> {
        let cutoff = unix_now() - interval_secs as i64;
        let counts = self
            .db
            .tool_invocation_counts_since(account, cutoff)
            .await
            .map_err(quota_db_err)?;
        let costs = self.db.quota_tool_costs(account).await.map_err(quota_db_err)?;
        let mut usage: u64 = 0;
        for (t, n) in counts {
            let Some(tn) = ToolName::from_name(&t) else { continue };
            if tn.direction() != dir {
                continue;
            }
            let c = costs.get(&t).copied().unwrap_or(DEFAULT_TOOL_COST);
            usage = usage.saturating_add(n.saturating_mul(c));
        }
        Ok(usage)
    }

    /// One-line quota summary for the direction the just-run tool charged
    /// against, prepended to its result so the caller can pace itself.
    /// Reflects usage AFTER this call's own deduction (it's read
    /// post-record). Best-effort: a db hiccup yields `None` and the
    /// response simply omits the header rather than failing an
    /// otherwise-successful tool call.
    async fn quota_header(&self, account: &str, dir: Direction) -> Option<Content> {
        let cfg = self.db.quota_config(account).await.ok()?;
        let (limit, interval) = cfg.for_direction(direction_to_quota(dir));
        let used = self.quota_used(account, dir, interval).await.ok()?;
        let available = limit.saturating_sub(used);
        let label = match dir {
            Direction::Read => "Read Quota",
            Direction::Write => "Write Quota",
        };
        Some(Content::text(format!("[{label}] {used} used, {available} available\n\n")))
    }
}

/// Map a tool's intrinsic direction to the db's quota-direction selector.
fn direction_to_quota(dir: Direction) -> QuotaDirection {
    match dir {
        Direction::Read => QuotaDirection::Read,
        Direction::Write => QuotaDirection::Write,
    }
}

/// Map a db-layer error into an rmcp `internal_error` for the quota path.
fn quota_db_err(e: psychological_operations_db::Error) -> ErrorData {
    ErrorData::internal_error(format!("quota: {e}"), None)
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
        Ok(ListToolsResult { tools, next_cursor: None, meta: None })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&context.extensions).await?;
        if is_hidden_for(state.mode, &request.name) {
            // Write tools aren't even listed in readonly mode, so reaching
            // one is a "should never happen" — a system fault, not an agent
            // mistake.
            return Err(ErrorData::internal_error(
                format!(
                    "tool '{}' is not available in readonly mode",
                    request.name
                ),
                None,
            ));
        }
        // Every metered tool carries a required `account` and is charged
        // against that account's read/write budget before it runs.
        // `read_queue`, `mark_handled`, and `list_accounts` are quota-free
        // — absent from `ToolName`, so they skip this gate (and the
        // usage header).
        let metered = match ToolName::from_name(&request.name) {
            Some(tool) => {
                // A missing `account` is the agent's mistake → hand it back
                // as an `is_error` tool result, not a protocol error.
                let account = match request
                    .arguments
                    .as_ref()
                    .and_then(|a| a.get("account"))
                    .and_then(|v| v.as_str())
                {
                    Some(a) => a.to_owned(),
                    None => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "tool '{}' requires an 'account' argument",
                            request.name,
                        ))]));
                    }
                };
                // Quota denial comes back as an `is_error` tool result we
                // hand straight to the agent — not a protocol error.
                if let Some(denied) = self.enforce_quota(&account, tool).await? {
                    return Ok(denied);
                }
                Some((account, tool.direction()))
            }
            None => None,
        };
        let tcc = ToolCallContext::new(self, request, context);
        let mut result = self.tool_router.call(tcc).await?;
        // Prepend the account's post-call quota usage for the direction
        // this tool charged against, so the caller can pace itself.
        if let Some((account, dir)) = metered {
            if let Some(header) = self.quota_header(&account, dir).await {
                result.content.insert(0, header);
            }
        }
        Ok(result)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        // No session context here — return the tool definition if
        // we have it. Mode-gating happens in `list_tools` /
        // `call_tool` where we DO have context.
        self.tool_router.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// The `ToolName` enum is the single source of truth for which tools
    /// are quota-metered. This guards against drift between it and the
    /// live tool router: every metered `ToolName` must be a real tool,
    /// and every router tool must be exactly one of metered XOR
    /// quota-free (the three exempt tools).
    #[test]
    fn tool_name_matches_router() {
        let router = PsychologicalOperationsXApiMcp::read_tools()
            + PsychologicalOperationsXApiMcp::write_tools()
            + PsychologicalOperationsXApiMcp::queue_tools()
            + PsychologicalOperationsXApiMcp::accounts_tools();
        let router_names: HashSet<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();

        // Tools that intentionally carry no quota.
        let quota_free: HashSet<&str> =
            ["list_accounts", "read_queue", "mark_handled"].into_iter().collect();

        // Every metered name is a real tool, and never quota-free.
        for tn in ToolName::ALL {
            assert!(
                router_names.contains(tn.as_name()),
                "ToolName::{tn:?} ('{}') is not a registered tool",
                tn.as_name(),
            );
            assert!(
                !quota_free.contains(tn.as_name()),
                "metered ToolName '{}' overlaps the quota-free set",
                tn.as_name(),
            );
        }

        // Every router tool is exactly one of metered / quota-free.
        for name in &router_names {
            let metered = ToolName::from_name(name).is_some();
            let free = quota_free.contains(name.as_str());
            assert!(
                metered ^ free,
                "router tool '{name}' must be exactly one of metered/quota-free \
                 (metered={metered}, free={free})",
            );
        }
    }
}
