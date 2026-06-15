//! Queue tools — read + dequeue the per-account ingest queue.
//!
//! These are DB-only (no X API call) and quota-free, but still take a
//! required `account` — the queue is keyed by account, so the tool reads
//! and dequeues the entries belonging to the identity named in the arg
//! (one of the names `list_accounts` returns).

use rmcp::model::{CallToolResult, Content};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::tool_error::{ToolError, finish};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {
    #[schemars(description = "Account whose queue to read — one of the names from list_accounts.")]
    pub account: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MarkHandledRequest {
    #[schemars(description = "Account whose queue to dequeue from — one of the names from list_accounts.")]
    pub account: String,
    #[schemars(
        description = "Numeric IDs of the tweets to remove from the queue."
    )]
    pub tweet_ids: Vec<String>,
}

#[tool_router(router = queue_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(
        name = "read_queue",
        description = "Read pending tweets from the queue."
    )]
    async fn read_queue(
        &self,
        Parameters(req): Parameters<ReadQueueRequest>,
    ) -> Result<String, ErrorData> {
        let entries = self
            .db
            .queue_list(&req.account)
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue list: {e}"), None))?;
        serde_json::to_string(&entries)
            .map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
    }

    #[tool(
        name = "mark_handled",
        description = "Remove one or more tweets from the queue."
    )]
    async fn mark_handled(
        &self,
        Parameters(req): Parameters<MarkHandledRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(async move {
            let missing = self.db.queue_delete_many(&req.account, &req.tweet_ids).await?;
            // Some id wasn't queued → the batch was rolled back (nothing
            // removed). That's the agent referencing items it can't
            // resolve → agent-facing.
            if !missing.is_empty() {
                return Err(ToolError::agent(format!(
                    "not in the queue for account '{}': {}. Nothing was removed (all-or-nothing).",
                    req.account,
                    missing.join(", "),
                )));
            }
            let body = serde_json::json!({ "removed": req.tweet_ids.len() }).to_string();
            Ok(CallToolResult::success(vec![Content::text(body)]))
        }.await)
    }
}
