//! `Destination::AgentQueue` — psyop delivery target that writes each
//! scored tweet into the SDK's per-agent queue. The target agent is
//! selected by `agent_tag` or `agent_instance_hierarchy` (untagged
//! [`AgentQueue`]); that choice also sets each row's `agent_kind`. Both
//! forms are used verbatim — no '/' collapsing.
//!
//! The queue is local SQLite (see
//! `psychological_operations_sdk::x::queue`); calling it doesn't hit the
//! X API. We open a Client purely to use its lazy `queue()` accessor.
//! `AuthMode::XApp` works fine — auth is never resolved for queue I/O.

pub use psychological_operations_sdk::cli::destinations::agent_queue::AgentQueue;

use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::{self, AgentKind, QueueEntry};

use super::Subject;

pub async fn send(
    cfg: &AgentQueue,
    subject: &Subject<'_>,
    ctx: &crate::context::Context,
) -> Result<(), crate::error::Error> {
    let Subject::Psyop { name, psyop: _, output } = subject;

    let (agent, agent_kind) = match cfg {
        AgentQueue::AgentTag { agent_tag } => (agent_tag.clone(), AgentKind::AgentTag),
        AgentQueue::AgentInstanceHierarchy { agent_instance_hierarchy } => {
            (agent_instance_hierarchy.clone(), AgentKind::AgentInstanceHierarchy)
        }
    };

    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        ctx.cache_max_size,
        ctx.cache_ttl,
        ctx.config.objectiveai_base_dir(),
        AuthMode::XApp,
    );
    let q = client
        .queue()
        .await
        .map_err(|e| crate::error::Error::Other(format!("queue open: {e}")))?;
    let now = queue::unix_now();

    for scored in *output {
        let entry = QueueEntry {
            agent:      agent.clone(),
            agent_kind,
            tweet_id:   scored.id.clone(),
            psyop:      Some((*name).to_string()),
            score:      Some(scored.score),
            deliverer_agent_instance_hierarchy: None,
            message:    None,
            queued_at:  now,
        };
        q.enqueue(&entry)
            .await
            .map_err(|e| crate::error::Error::Other(format!("queue enqueue: {e}")))?;
    }
    Ok(())
}
