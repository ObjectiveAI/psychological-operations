//! `agents enqueue x --agent-tag <tag> --tweet-id <id> --message <msg>` —
//! the operator flags a tweet for an agent's X queue, then the agent is woken
//! immediately (`agents message`) with its new pending count.
//!
//! The queue is per-agent (caller-agnostic). Row shape: `message =
//! Some(msg)`, the caller's `deliverer_agent_instance_hierarchy` (from
//! `OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY`, verbatim, as provenance), no
//! `psyop` / `score` / `run_id`.

use psychological_operations_db::{unix_now, XQueueEntry};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

pub async fn run(
    agent_tag: &str,
    tweet_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent_tag, tweet_id, message, ctx).await)
}

async fn run_inner(
    agent_tag: &str,
    tweet_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    ctx.db
        .x_queue_enqueue(&XQueueEntry {
            agent_tag: agent_tag.to_string(),
            tweet_id: tweet_id.to_string(),
            psyop: None,
            score: None,
            deliverer_agent_instance_hierarchy: Some(
                ctx.config.objectiveai_agent_instance_hierarchy.clone(),
            ),
            message: Some(message.to_string()),
            run_id: None,
            queued_at: unix_now(),
        })
        .await
        .map_err(|e| Error::Other(format!("x queue enqueue: {e}")))?;

    // Wake the agent now with its new pending counts (manual enqueue → instant
    // `agents message`, not the batched park-then-deliver path).
    super::super::notify::message_agent(&ctx.db, &ctx.executor, agent_tag).await?;

    Ok(Output::Ok)
}
