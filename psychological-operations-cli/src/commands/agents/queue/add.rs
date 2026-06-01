//! `agents queue add --tweet-id <id> --message <msg>` — the caller
//! flags a tweet for the agent named in `OBJECTIVEAI_AGENT_ID_BASE`.
//!
//! The queue itself is per-agent (caller-agnostic). Row shape:
//! `deliverer = Some(agent)`, `message = Some(msg)`, no `psyop` /
//! `score`.

use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::{self, QueueEntry};

use crate::error::Error;

pub async fn run(
    tweet_id: &str,
    message: &str,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let agent = cfg
        .objectiveai_agent_id_base
        .as_deref()
        .ok_or_else(|| {
            Error::Other(
                "OBJECTIVEAI_AGENT_ID_BASE not set — required for `agents queue add`"
                    .into(),
            )
        })?;

    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        256 * 1024 * 1024,
        std::time::Duration::from_secs(3600),
        cfg.objectiveai_base_dir(),
        AuthMode::XApp,
    );
    let q = client
        .queue()
        .await
        .map_err(|e| Error::Other(format!("queue open: {e}")))?;

    q.enqueue(&QueueEntry {
        agent:     agent.to_string(),
        tweet_id:  tweet_id.to_string(),
        psyop:     None,
        score:     None,
        deliverer: Some(agent.to_string()),
        message:   Some(message.to_string()),
        queued_at: queue::unix_now(),
    })
    .await
    .map_err(|e| Error::Other(format!("queue enqueue: {e}")))?;

    Ok(crate::Output::Empty)
}
