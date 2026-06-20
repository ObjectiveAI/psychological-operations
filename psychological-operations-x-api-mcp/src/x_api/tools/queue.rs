//! Queue tools — read + dequeue the per-agent ingest queue.
//!
//! These are DB-only (no X API call) and quota-free. The queue is keyed
//! by agent tag, so the tools read + dequeue the entries belonging to the
//! session's `tag` (from the `X-OBJECTIVEAI-ARGUMENTS` header).

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::tool_error::{ToolError, finish};
use super::read::{check_count, remaining_note};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {
    #[schemars(description = "How many queued tweets to return (window size; max 100).")]
    pub count: u32,
    #[schemars(description = "How many queued tweets to skip before the window.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MarkHandledRequest {
    #[schemars(description = "Numeric IDs of the tweets to remove from the queue.")]
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
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let entries = self.db.queue_list(&tag).await?;
                // The whole per-agent queue is in hand, so `remaining` is
                // exact (never `over `): this is a complete DB list, not a
                // cursor source.
                let note = remaining_note(
                    entries.len(),
                    req.offset as usize,
                    req.count as usize,
                    false,
                );
                let window: Vec<_> = entries
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .collect();
                let body = serde_json::to_string(&window)?;
                Ok(CallToolResult::success(vec![
                    Content::text(body),
                    Content::text(note),
                ]))
            }
            .await,
        )
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
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let missing = self.db.queue_delete_many(&tag, &req.tweet_ids).await?;
                // Some id wasn't queued → the batch was rolled back (nothing
                // removed). That's the agent referencing items it can't
                // resolve → agent-facing.
                if !missing.is_empty() {
                    return Err(ToolError::agent(format!(
                        "not in the queue for tag '{}': {}. Nothing was removed (all-or-nothing).",
                        tag,
                        missing.join(", "),
                    )));
                }
                let body = serde_json::json!({ "removed": req.tweet_ids.len() }).to_string();
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }
}
