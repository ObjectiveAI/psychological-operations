//! Queue tools — read + dequeue the per-account ingest queue.
//!
//! These are DB-only (no X API call) and quota-free. The queue is keyed
//! by account, so the tools read + dequeue the entries belonging to the
//! session's `account` (from the `X-OBJECTIVEAI-ARGUMENTS` header).

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::tool_error::{ToolError, finish};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MarkHandledRequest {
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
        Parameters(_req): Parameters<ReadQueueRequest>,
        extensions: Extensions,
    ) -> Result<String, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        let entries = self
            .db
            .queue_list(&account)
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
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let missing = self.db.queue_delete_many(&account, &req.tweet_ids).await?;
            // Some id wasn't queued → the batch was rolled back (nothing
            // removed). That's the agent referencing items it can't
            // resolve → agent-facing.
            if !missing.is_empty() {
                return Err(ToolError::agent(format!(
                    "not in the queue for account '{}': {}. Nothing was removed (all-or-nothing).",
                    account,
                    missing.join(", "),
                )));
            }
            let body = serde_json::json!({ "removed": req.tweet_ids.len() }).to_string();
            Ok(CallToolResult::success(vec![Content::text(body)]))
        }.await)
    }
}
