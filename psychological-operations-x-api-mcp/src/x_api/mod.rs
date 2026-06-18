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
//! `tag`, `mode`, and the per-session `quota_*` overrides are NOT
//! process-wide. They land here per session, sourced from the
//! `X-OBJECTIVEAI-ARGUMENTS` JSON-object header on every request —
//! recorded by [`crate::header_session_manager::HeaderSessionManager`]
//! into [`session::SessionRegistry`] keyed by `Mcp-Session-Id`. Tool
//! handlers look them up via [`PsychologicalOperationsXApiMcp::resolve_session`]
//! and build a fresh SDK [`Client`] for each call via
//! [`PsychologicalOperationsXApiMcp::build_client`].

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
use psychological_operations_db::{Db, unix_now};
use psychological_operations_sdk::x::client::Client;
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

use crate::Mode;
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

/// For a `reply` / `quote` call, pull `(kind, target_tweet_id)` out of the
/// raw arguments so the pending-duplicate pre-check can run in `call_tool`
/// before quota is charged. `None` for any other tool.
fn reply_quote_target(request: &CallToolRequestParams) -> Option<(&'static str, String)> {
    let (kind, arg) = match request.name.as_ref() {
        "reply" => ("reply", "in_reply_to_tweet_id"),
        "quote" => ("quote", "quote_tweet_id"),
        _ => return None,
    };
    let target = request.arguments.as_ref()?.get(arg)?.as_str()?.to_string();
    Some((kind, target))
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
    ) -> Self {
        Self {
            tool_router: Self::read_tools() + Self::write_tools() + Self::queue_tools(),
            sessions,
            reqwest,
            state_dir,
            db,
            cache_max_size,
            cache_ttl,
            mock,
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
                ErrorData::invalid_params(format!("missing {HEADER_SESSION_ID} header"), None)
            })?;
        self.sessions
            .get(&id.to_owned().into())
            .await
            .ok_or_else(|| ErrorData::invalid_params(format!("unknown session: {id}"), None))
    }

    /// Build a fresh SDK [`Client`], reusing the process-wide reqwest
    /// pool, cache config, state dir, and `db` handle. Called once per
    /// tool invocation — `Client::new` is infallible + synchronous. The
    /// identity to act as is no longer baked in here: each tool builds
    /// `AuthMode::Agent(state.tag)` from the session's `tag`
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
        state: &SessionState,
        tool: ToolName,
    ) -> Result<Option<CallToolResult>, ErrorData> {
        let dir = tool.direction();
        let (used, limit) = self.quota_status(state, dir).await?;
        // Deny only when usage is already at/above the (grant-boosted)
        // limit. The call's own cost doesn't gate it — a single call may
        // push usage past the limit (which then blocks the *next* same-
        // direction call), so an expensive tool stays usable as long as
        // there's any headroom left.
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

    /// Windowed usage + grant-boosted limit for one direction, fetched in
    /// one place. The usage ledger read and the active-grants read are
    /// independent, so they run **concurrently** (`tokio::join!`).
    ///
    /// - usage = Σ (count × per-tool cost) over same-direction invocations
    ///   newer than `now - interval` (unknown tool names ignored).
    /// - limit = the session's per-direction base + the total of all grants
    ///   for `(tag, direction)` in effect right now.
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
            self.db.active_quota_grants(&state.tag, dir_str, now),
        );
        let counts = counts.map_err(quota_db_err)?;
        let grants = grants.map_err(quota_db_err)?.max(0) as u64;
        let used = sum_usage(counts, dir, &state.quota_tool_costs);
        Ok((used, base.saturating_add(grants)))
    }

    /// One-line quota summary for the direction the just-run tool charged
    /// against, prepended to its result so the caller can pace itself.
    /// Reflects usage AFTER this call's own deduction (it's read
    /// post-record). Best-effort: a db hiccup yields `None` and the
    /// response simply omits the header rather than failing an
    /// otherwise-successful tool call.
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

/// Σ (count × per-tool cost) over the account's same-direction
/// invocations. Unknown tool names (drift / renamed tools) are ignored.
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
            // Write tools aren't even listed in readonly mode, so reaching
            // one is a "should never happen" — a system fault, not an agent
            // mistake.
            return Err(ErrorData::internal_error(
                format!("tool '{}' is not available in readonly mode", request.name),
                None,
            ));
        }
        // Reply/quote: refuse a duplicate while one is already pending
        // delivery (a reply blocks only a reply, a quote only a quote).
        // MUST run BEFORE enforce_quota so a pending-block never consumes
        // quota. A successful queue (in the tool body, on the 403) happens
        // after quota was charged, so that path keeps charging.
        if let Some((kind, target)) = reply_quote_target(&request) {
            if self
                .db
                .reply_quote_pending_exists(&state.tag, kind, &target)
                .await
                .unwrap_or(false)
            {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "A {kind} for tweet {target} is already pending delivery — \
                     not submitting another."
                ))]));
            }
        }
        // Metered tools (those in `ToolName`) are charged against the
        // session's account before they run; `read_queue` + `mark_handled`
        // are quota-free (absent from `ToolName`), so they skip the gate
        // and the usage header. The acting account comes from the session.
        let metered = match ToolName::from_name(&request.name) {
            Some(tool) => {
                // Quota denial comes back as an `is_error` tool result we
                // hand straight to the agent — not a protocol error.
                if let Some(denied) = self.enforce_quota(&state, tool).await? {
                    return Ok(denied);
                }
                Some(tool.direction())
            }
            None => None,
        };
        let tcc = ToolCallContext::new(self, request, context);
        let mut result = self.tool_router.call(tcc).await?;
        // Prepend the account's post-call quota usage for the direction
        // this tool charged against, so the caller can pace itself.
        if let Some(dir) = metered {
            if let Some(header) = self.quota_header(&state, dir).await {
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
            + PsychologicalOperationsXApiMcp::queue_tools();
        let router_names: HashSet<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();

        // Tools that intentionally carry no quota.
        let quota_free: HashSet<&str> = ["read_queue", "mark_handled"].into_iter().collect();

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
