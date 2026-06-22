//! Agent queue-count notification.
//!
//! After delivering tweets to an agent's queue (psyops run, or a manual
//! `agents enqueue`), we park an objectiveai `agents enqueue` message
//! telling the agent how many tweets are now waiting. The row is keyed
//! `"psychological-operations"` so re-notifying replaces the prior count
//! rather than stacking duplicates; it parks the notification without
//! spawning the agent (delivery happens on a later `agents queue
//! deliver`). The agent is always addressed by its tag.

use objectiveai_sdk::cli::command::agents::enqueue as agents_enqueue;
use objectiveai_sdk::cli::command::agents::message::RequestMessage;
use objectiveai_sdk::cli::command::agents::selector::AgentSelector;

use crate::error::Error;

/// Park an `agents enqueue` notification for `agent_tag` reporting its
/// current pending-queue count. Idempotent-replace (keyed
/// `"psychological-operations"`).
pub async fn notify_agent(ctx: &crate::context::Context, agent_tag: &str) -> Result<(), Error> {
    let n = ctx
        .db
        .queue_count(agent_tag)
        .await
        .map_err(|e| Error::Other(format!("queue count: {e}")))?;
    let request = agents_enqueue::Request {
        path_type: agents_enqueue::Path::AgentsEnqueue,
        agent: AgentSelector::Tag {
            agent_tag: agent_tag.to_string(),
        },
        message: RequestMessage::Simple(format!(
            "<psychological-operations>\nThe agent \"{agent_tag}\" has {n} tweets in the queue.\n</psychological-operations>"
        )),
        key: Some("psychological-operations".to_string()),
        base: Default::default(),
    };
    agents_enqueue::execute(&*ctx.executor, request, None)
        .await
        .map_err(|e| Error::Other(format!("notify {agent_tag}: {e}")))?;
    Ok(())
}
