//! `Destination::Queue { agent }` — psyop delivery target that
//! writes each scored tweet into the SDK's per-agent queue.
//!
//! The Queue is local SQLite (see
//! `psychological_operations_sdk::x::queue`); calling it doesn't
//! hit the X API. We open a Client purely to use its lazy
//! `queue()` accessor. `AuthMode::XApp` works fine — auth is
//! never resolved for queue I/O.

pub use psychological_operations_sdk::cli::destinations::queue::Queue;

use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::{self, AgentKind, QueueEntry};

use super::Subject;

pub async fn send(
    cfg: &Queue,
    subject: &Subject<'_>,
    ctx: &crate::context::Context,
) -> Result<(), crate::error::Error> {
    let Subject::Psyop { name, psyop: _, output } = subject;

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
            agent:      cfg.agent.clone(),
            // TODO(broader refactor): the psyop Queue destination's
            // `agent` is an operator-configured name; treat it as a tag
            // for now. Revisit when the destination config carries kind.
            agent_kind: AgentKind::AgentTag,
            tweet_id:   scored.post.id.clone(),
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
