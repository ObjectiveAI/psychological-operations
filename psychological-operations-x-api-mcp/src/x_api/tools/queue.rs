//! Queue tools — read + dequeue the per-agent ingest queue.

use rmcp::model::Extensions;
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
        extensions: Extensions,
    ) -> Result<String, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        // Queue tools are DB-only — no X API call, so the persona auth
        // is unused here.
        let (http, _auth) = self.build_client(&state);

        let entries = http
            .db()
            .queue_list(&state.agent)
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
        extensions: Extensions,
    ) -> Result<String, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        // Queue tools are DB-only — no X API call, so the persona auth
        // is unused here.
        let (http, _auth) = self.build_client(&state);

        let removed = http
            .db()
            .queue_delete(&state.agent, &req.tweet_id)
            .await
            .map_err(|e| ErrorData::internal_error(format!("queue delete: {e}"), None))?;
        Ok(serde_json::json!({ "removed": removed }).to_string())
    }
}
