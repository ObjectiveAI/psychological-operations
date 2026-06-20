//! Queue tools — read + dequeue the per-agent ingest queue.
//!
//! These are DB-only (no X API call) and quota-free. The queue is keyed
//! by agent tag, so the tools read + dequeue the entries belonging to the
//! session's `tag` (from the `X-OBJECTIVEAI-ARGUMENTS` header).
//!
//! `read_queue` reshapes the raw rows into agent-facing **items**: every
//! row a single psyop run enqueued (they share a `run_id`) collapses into
//! one psyop-group item carrying all its `(tweet_id, score)` pairs; each
//! operator-flagged row (`agents enqueue`, no `run_id`) is its own item.
//! `count`/`offset` window the item list. The agent's own `agent_tag` is
//! never echoed, and `queued_at` is rendered RFC 3339.

use std::collections::HashMap;

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::tool_error::{ToolError, finish};
use super::read::{check_count, remaining_note};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadQueueRequest {
    #[schemars(description = "How many queue items to return (window size; max 100). \
                             A psyop run's tweets count as a single item.")]
    pub count: u32,
    #[schemars(description = "How many queue items to skip before the window.")]
    pub offset: u32,
}

/// One psyop run's worth of queued tweets, collapsed into a single item.
#[derive(serde::Serialize)]
struct PsyopGroup {
    psyop: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deliverer_agent_instance_hierarchy: Option<String>,
    queued_at: String,
    /// The psyop's `message`, looked up by name at read time (not stored on
    /// the rows); absent if the psyop is gone or has no message.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// `[tweet_id, score]` pairs for every tweet this run delivered.
    tweets: Vec<(String, f64)>,
}

/// A single operator-flagged tweet (`agents enqueue`).
#[derive(serde::Serialize)]
struct OperatorItem {
    tweet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deliverer_agent_instance_hierarchy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    queued_at: String,
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum QueueItem {
    Psyop(PsyopGroup),
    Operator(OperatorItem),
}

/// Render a unix-seconds timestamp as an RFC 3339 string (empty on the
/// impossible out-of-range case).
fn rfc3339(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
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

                // Collapse the rows into items: rows sharing a `run_id` fold
                // into one psyop group (encounter order, which is queued_at
                // ASC); operator rows (no `run_id`) stay individual.
                let mut items: Vec<QueueItem> = Vec::new();
                let mut group_idx: HashMap<String, usize> = HashMap::new();
                // psyop name -> its `message`, cached so we hit the DB once
                // per distinct psyop per call.
                let mut msg_cache: HashMap<String, Option<String>> = HashMap::new();

                for e in entries {
                    match e.run_id {
                        Some(run_id) => {
                            let tweet = (e.tweet_id, e.score.unwrap_or(0.0));
                            if let Some(&i) = group_idx.get(&run_id) {
                                if let QueueItem::Psyop(g) = &mut items[i] {
                                    g.tweets.push(tweet);
                                }
                                continue;
                            }
                            let psyop = e.psyop.unwrap_or_default();
                            let message = match msg_cache.get(&psyop) {
                                Some(m) => m.clone(),
                                None => {
                                    let m = self
                                        .db
                                        .psyop_get(&psyop)
                                        .await?
                                        .as_ref()
                                        .and_then(|v| v.get("message"))
                                        .and_then(|m| m.as_str())
                                        .map(str::to_string);
                                    msg_cache.insert(psyop.clone(), m.clone());
                                    m
                                }
                            };
                            group_idx.insert(run_id, items.len());
                            items.push(QueueItem::Psyop(PsyopGroup {
                                psyop,
                                deliverer_agent_instance_hierarchy: e
                                    .deliverer_agent_instance_hierarchy,
                                queued_at: rfc3339(e.queued_at),
                                message,
                                tweets: vec![tweet],
                            }));
                        }
                        None => items.push(QueueItem::Operator(OperatorItem {
                            tweet_id: e.tweet_id,
                            deliverer_agent_instance_hierarchy: e.deliverer_agent_instance_hierarchy,
                            message: e.message,
                            queued_at: rfc3339(e.queued_at),
                        })),
                    }
                }

                // The whole per-agent queue is in hand, so `remaining` is
                // exact (never `over `): a complete DB list, not a cursor.
                let note =
                    remaining_note(items.len(), req.offset as usize, req.count as usize, false);
                let window: Vec<QueueItem> = items
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
