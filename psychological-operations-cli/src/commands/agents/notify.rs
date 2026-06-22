//! Agent queue-count notification.
//!
//! After delivering to an agent's queues (psyops run, or a manual
//! `agents enqueue`), we park an objectiveai `agents enqueue` message telling
//! the agent how many items are now waiting across both the X and Discord
//! queues. The row is keyed `"psychological-operations"` so re-notifying
//! replaces the prior count rather than stacking duplicates; it parks the
//! notification without spawning the agent. The agent is addressed by its tag.

use objectiveai_sdk::cli::command::agents::enqueue as agents_enqueue;
use objectiveai_sdk::cli::command::agents::message::RequestMessage;
use objectiveai_sdk::cli::command::agents::selector::AgentSelector;

use crate::error::Error;

/// Park an `agents enqueue` notification for `agent_tag` reporting its current
/// pending counts across both queues. Counts are queried in parallel; if both
/// are zero, nothing is parked. The message lists only the non-empty queues.
/// Idempotent-replace (keyed `"psychological-operations"`).
pub async fn notify_agent(ctx: &crate::context::Context, agent_tag: &str) -> Result<(), Error> {
    let (x, discord) = tokio::try_join!(
        ctx.db.x_queue_count(agent_tag),
        ctx.db.discord_queue_count(agent_tag),
    )
    .map_err(|e| Error::Other(format!("queue count: {e}")))?;

    if x == 0 && discord == 0 {
        return Ok(());
    }

    let mut lines = String::new();
    if x > 0 {
        lines.push_str(&format!("[x] {x} tweets in the queue.\n"));
    }
    if discord > 0 {
        lines.push_str(&format!("[discord] {discord} messages in the queue.\n"));
    }
    let message = format!("<psychological-operations>\n{lines}</psychological-operations>");

    let request = agents_enqueue::Request {
        path_type: agents_enqueue::Path::AgentsEnqueue,
        agent: AgentSelector::Tag {
            agent_tag: agent_tag.to_string(),
        },
        message: RequestMessage::Simple(message),
        key: Some("psychological-operations".to_string()),
        base: Default::default(),
    };
    agents_enqueue::execute(&*ctx.executor, request, None)
        .await
        .map_err(|e| Error::Other(format!("notify {agent_tag}: {e}")))?;
    Ok(())
}
