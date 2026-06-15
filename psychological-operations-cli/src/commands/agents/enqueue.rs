//! `agents enqueue <agent-selector> --tweet-id <id> --message <msg>` —
//! the caller flags a tweet for the agent named by the shared
//! `--agent-tag` / `--me` / `--agent-instance` selector (resolved by the
//! caller; this module receives the final agent name).
//!
//! The queue itself is per-agent (caller-agnostic). Row shape:
//! `agent_kind` from the selector, `message = Some(msg)`, the caller's
//! `deliverer_agent_instance_hierarchy` (straight from
//! `OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY`, verbatim), no `psyop` /
//! `score`.

use psychological_operations_db::{AgentKind, QueueEntry, unix_now};
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::Client;

use crate::error::Error;

pub async fn run(
    agent: &str,
    agent_kind: AgentKind,
    tweet_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent, agent_kind, tweet_id, message, ctx).await)
}

async fn run_inner(
    agent: &str,
    agent_kind: AgentKind,
    tweet_id: &str,
    message: &str,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let client = Client::new(
        reqwest::Client::new(),
        ctx.config.mock,
        ctx.cache_max_size,
        ctx.cache_ttl,
        ctx.config.state_dir(),
        ctx.db.clone(),
    );

    client.db().queue_enqueue(&QueueEntry {
        agent:      agent.to_string(),
        agent_kind,
        tweet_id:   tweet_id.to_string(),
        psyop:      None,
        score:      None,
        deliverer_agent_instance_hierarchy: Some(
            ctx.config.objectiveai_agent_instance_hierarchy.clone(),
        ),
        message:    Some(message.to_string()),
        queued_at:  unix_now(),
    })
    .await
    .map_err(|e| Error::Other(format!("queue enqueue: {e}")))?;

    Ok(Output::Ok)
}
