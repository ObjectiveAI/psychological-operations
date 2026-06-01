//! `agents enqueue --tweet-id <id> --message <msg>` —
//! operator-driven queue insert. Reads the current agent from
//! `OBJECTIVEAI_AGENT_ID_BASE` (via `Config`) and writes a row
//! with `deliverer = Some(agent)` + `message = Some(msg)` and no
//! `psyop` / `score`.

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
                "OBJECTIVEAI_AGENT_ID_BASE not set — required for `agents enqueue`"
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
