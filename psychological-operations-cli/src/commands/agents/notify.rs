//! Agent queue-count notification.
//!
//! After delivering to an agent's queues (psyops run, or a daemon hook match),
//! we tell the agent how many items are now waiting across the X, Discord, and
//! Twitch queues. Two delivery shapes share one summary builder:
//!
//! * [`notify_agent`] parks an `agents enqueue` message keyed
//!   `"psychological-operations"` (re-notifying replaces the prior count rather
//!   than stacking) **without** spawning the agent — the batched psyop/hook
//!   path, woken later by a single `agents queue deliver --key …`.
//! * [`message_agent`] delivers via `agents message`, waking the agent
//!   immediately — the manual `agents enqueue x|discord` path.
//!
//! The agent is addressed by its tag in both cases.

use std::sync::Arc;

use objectiveai_sdk::cli::command::agents::enqueue as agents_enqueue;
use objectiveai_sdk::cli::command::agents::message::{self as agents_message, RequestMessage};
use objectiveai_sdk::cli::command::agents::selector::AgentSelector;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use psychological_operations_db::Db;

use crate::error::Error;

/// The objectiveai-queue `key` every psyop/hook notification is parked under.
/// Re-notifying replaces the prior count (idempotent), and the daemon's
/// `agents queue deliver` scopes to exactly this key so it only delivers our
/// own parked notifications.
pub const NOTIFY_KEY: &str = "psychological-operations";

/// Park an `agents enqueue` notification for `agent_tag` reporting its current
/// pending counts across all queues. Counts are queried in parallel; if all
/// are zero, nothing is parked. The message lists only the non-empty queues.
/// Idempotent-replace (keyed `"psychological-operations"`).
///
/// Takes the `db` + `executor` directly (not a `Context`) so callers without a
/// full context — notably the daemon's gateway hook handler — can park
/// notifications too.
pub async fn notify_agent(
    db: &Db,
    executor: &Arc<PluginExecutor>,
    agent_tag: &str,
) -> Result<(), Error> {
    let Some(message) = pending_summary(db, agent_tag).await? else {
        return Ok(());
    };

    let request = agents_enqueue::Request {
        path_type: agents_enqueue::Path::AgentsEnqueue,
        agent: AgentSelector::Tag {
            agent_tag: agent_tag.to_string(),
        },
        message: RequestMessage::Simple(message),
        key: Some(NOTIFY_KEY.to_string()),
        base: Default::default(),
    };
    agents_enqueue::execute(&**executor, request, None)
        .await
        .map_err(|e| Error::Other(format!("notify {agent_tag}: {e}")))?;
    Ok(())
}

/// Deliver the same pending-count summary to `agent_tag` via `agents message`
/// — waking the agent **now** (continue its live hierarchy, or spawn it if its
/// tag is grouped/dormant) instead of parking a keyed notification for a later
/// `agents queue deliver`. Used by the manual `agents enqueue x|discord` path,
/// which wants an immediate one-shot wake rather than batched delivery. If all
/// queues are empty, nothing is sent.
pub async fn message_agent(
    db: &Db,
    executor: &Arc<PluginExecutor>,
    agent_tag: &str,
) -> Result<(), Error> {
    let Some(message) = pending_summary(db, agent_tag).await? else {
        return Ok(());
    };

    let request = agents_message::Request {
        path_type: agents_message::Path::AgentsMessage,
        agent: AgentSelector::Tag {
            agent_tag: agent_tag.to_string(),
        },
        message: RequestMessage::Simple(message),
        dangerous_advanced: None,
        base: Default::default(),
    };
    agents_message::execute(&**executor, request, None)
        .await
        .map_err(|e| Error::Other(format!("message {agent_tag}: {e}")))?;
    Ok(())
}

/// Build the pending-count summary for `agent_tag`: query all queues in
/// parallel and render `<psychological-operations>`-wrapped lines for the
/// non-empty ones. Returns `None` when all queues are empty (caller sends
/// nothing). Shared by [`notify_agent`] (park) and [`message_agent`] (wake).
async fn pending_summary(db: &Db, agent_tag: &str) -> Result<Option<String>, Error> {
    let (x, discord, twitch) = tokio::try_join!(
        db.x_queue_count(agent_tag),
        db.discord_queue_count(agent_tag),
        db.twitch_queue_count(agent_tag),
    )
    .map_err(|e| Error::Other(format!("queue count: {e}")))?;

    if x == 0 && discord == 0 && twitch == 0 {
        return Ok(None);
    }

    let mut lines = String::new();
    if x > 0 {
        lines.push_str(&format!("[x] {x} tweets in the queue.\n"));
    }
    if discord > 0 {
        lines.push_str(&format!("[discord] {discord} messages in the queue.\n"));
    }
    if twitch > 0 {
        lines.push_str(&format!("[twitch] {twitch} messages in the queue.\n"));
    }
    Ok(Some(format!(
        "<psychological-operations>\n{lines}</psychological-operations>"
    )))
}
