//! Queue tools — read + dequeue the per-account ingest queue.
//!
//! These are DB-only (no X API call) and quota-free, but still take a
//! required `account` — the queue is keyed by account, so the tool reads
//! and dequeues the entries belonging to the identity named in the arg
//! (one of the names `list_accounts` returns).

use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {
    #[schemars(description = "Account whose queue to read — one of the names from list_accounts.")]
    pub account: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MarkHandledRequest {
    #[schemars(description = "Account whose queue to dequeue from — one of the names from list_accounts.")]
    pub account: String,
    #[schemars(description = "Numeric ID of the tweet to remove from the queue.")]
    pub tweet_id: String,
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
        description = "Remove a tweet from the queue."
    )]
    async fn mark_handled(
        &self,
        Parameters(req): Parameters<MarkHandledRequest>,
    ) -> Result<String, ErrorData> {
        let removed = self
            .db
            .queue_delete(&req.account, &req.tweet_id)
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue delete: {e}"), None))?;
        Ok(serde_json::json!({ "removed": removed }).to_string())
    }
}
