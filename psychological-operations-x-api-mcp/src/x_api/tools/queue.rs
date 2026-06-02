//! Queue tools — read + dequeue the per-agent ingest queue.

use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MarkHandledRequest {
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
        Parameters(_req): Parameters<ReadQueueRequest>,
    ) -> Result<String, ErrorData> {
        let q = self
            .http
            .queue()
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue open: {e}"), None))?;
        let entries = q
            .list(&self.agent)
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
        let q = self
            .http
            .queue()
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue open: {e}"), None))?;
        let removed = q
            .delete(&self.agent, &req.tweet_id)
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue delete: {e}"), None))?;
        Ok(serde_json::json!({ "removed": removed }).to_string())
    }
}
