//! `agents enqueue discord --agent-tag <tag> --channel-id <c> --message-id <m>
//! --message <msg>` — the operator flags a Discord message for an agent's
//! Discord queue, then the agent is auto-notified of its new pending count.
//!
//! A Discord message is fully keyed by `(channel_id, message_id)`. Row shape
//! mirrors the X enqueue: `message = Some(msg)`, the caller's
//! `deliverer_agent_instance_hierarchy`, no `psyop` / `score` / `run_id`.

use psychological_operations_db::{unix_now, DiscordQueueEntry};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

pub async fn run(
    agent_tag: &str,
    channel_id: &str,
    message_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent_tag, channel_id, message_id, message, ctx).await)
}

async fn run_inner(
    agent_tag: &str,
    channel_id: &str,
    message_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    ctx.db
        .discord_queue_enqueue(&DiscordQueueEntry {
            agent_tag: agent_tag.to_string(),
            channel_id: channel_id.to_string(),
            message_id: message_id.to_string(),
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
        .map_err(|e| Error::Other(format!("discord queue enqueue: {e}")))?;

    // Auto-notify the agent of its new pending counts (both queues).
    super::super::notify::notify_agent(ctx, agent_tag).await?;

    Ok(Output::Ok)
}
