//! `Destination::Queue { agent }` — psyop delivery target that
//! writes each scored tweet into the SDK's per-agent queue.
//!
//! The Queue is local SQLite (see
//! `psychological_operations_sdk::x::queue`); calling it doesn't
//! hit the X API. We open a Client purely to use its lazy
//! `queue()` accessor. `AuthMode::XApp` works fine — auth is
//! never resolved for queue I/O.

use serde::{Deserialize, Serialize};

use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::{self, QueueEntry};

use super::Subject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queue {
    /// Target agent name. Each scored tweet lands in this
    /// agent's queue (PRIMARY KEY (agent, tweet_id)).
    pub agent: String,
}

pub async fn send(
    cfg: &Queue,
    subject: &Subject<'_>,
    rt: &crate::run::Config,
) -> Result<(), crate::error::Error> {
    let Subject::Psyop { name, psyop: _, output } = subject;

    let operator = rt
        .objectiveai_agent_id
        .as_deref()
        .ok_or_else(|| {
            crate::error::Error::Other(
                "OBJECTIVEAI_AGENT_ID not set — required to enqueue to a Queue destination"
                    .into(),
            )
        })?;

    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        256 * 1024 * 1024,
        std::time::Duration::from_secs(3600),
        rt.objectiveai_base_dir(),
        AuthMode::XApp,
    );
    let q = client
        .queue()
        .await
        .map_err(|e| crate::error::Error::Other(format!("queue open: {e}")))?;
    let now = queue::unix_now();

    for scored in *output {
        let entry = QueueEntry {
            objectiveai_agent_id: operator.to_string(),
            agent:                cfg.agent.clone(),
            tweet_id:             scored.post.id.clone(),
            psyop:                Some((*name).to_string()),
            score:                Some(scored.score),
            deliverer:            None,
            message:              None,
            queued_at:            now,
        };
        q.enqueue(&entry)
            .await
            .map_err(|e| crate::error::Error::Other(format!("queue enqueue: {e}")))?;
    }
    Ok(())
}
