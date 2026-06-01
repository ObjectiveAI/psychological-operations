//! `agents queue handle` — parallel per-agent dispatcher.
//!
//! Walks the operator's queue, picks (or spawns) one objectiveai
//! handler agent per X-API agent, and runs all the per-agent tasks
//! concurrently via a `JoinSet`. Each task that ends up actually
//! handling work (queue ≥ 1) emits a single notification:
//!
//! ```json
//! {"event":"agent_queue_handled","agent":"<name>","handling":<n>,"error":<null|msg>}
//! ```
//!
//! Agents with no queue items are skipped silently — no notification.
//! Errors are emitted twice: once as a regular plugin-output Error
//! (so the standard error surface shows it) AND once on the
//! notification's `error` field.
//!
//! Per-agent handing-off:
//! 1. Look up the stored objectiveai handler id for
//!    `(operator, agent)` in `handler_map`.
//! 2. If we have one, plugin-dispatch `agents message <handler>` with
//!    a short prompt.
//! 3. On failure, plugin-dispatch `agents list active`; if the stored
//!    handler is still in the list, the message failure was real (we
//!    propagate). Otherwise the handler is gone — fall through.
//! 4. Plugin-dispatch `agents spawn` with the full handler prompt.
//!    Persist the returned `Spawned.agent_id` as the new handler.

use std::sync::Arc;
use std::time::Duration;

use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::Queue;
use tokio::task::JoinSet;

use crate::error::Error;

use super::dispatch;

pub async fn run(
    agent_filter: Vec<String>,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let operator = cfg
        .objectiveai_agent_id
        .as_deref()
        .ok_or_else(|| {
            Error::Other(
                "OBJECTIVEAI_AGENT_ID not set — required for `agents queue handle`"
                    .into(),
            )
        })?
        .to_string();

    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        256 * 1024 * 1024,
        Duration::from_secs(3600),
        cfg.objectiveai_base_dir(),
        AuthMode::XApp,
    );
    let q: Arc<Queue> = client
        .queue()
        .await
        .map_err(|e| Error::Other(format!("queue open: {e}")))?
        .clone();

    let mut agents = q
        .list_agents_with_entries(&operator)
        .await
        .map_err(|e| Error::Other(format!("list agents: {e}")))?;
    if !agent_filter.is_empty() {
        agents.retain(|a| agent_filter.iter().any(|f| f == a));
    }

    let mut tasks = JoinSet::new();
    for agent in agents {
        let operator = operator.clone();
        let q = q.clone();
        tasks.spawn(async move {
            let res = handle_one_agent(&operator, &agent, &q).await;
            emit_per_agent(&agent, res);
        });
    }
    while tasks.join_next().await.is_some() {}

    Ok(crate::Output::Empty)
}

async fn handle_one_agent(
    operator: &str,
    agent: &str,
    q: &Queue,
) -> Result<usize, (usize, String)> {
    let entries = q
        .list(operator, agent)
        .await
        .map_err(|e| (0usize, format!("queue list: {e}")))?;
    if entries.is_empty() {
        return Ok(0);
    }
    let n = entries.len();
    let key_prefix = format!("{operator}::{agent}");

    // Step 1: try messaging the stored handler.
    let stored = q
        .get_handler(operator, agent)
        .await
        .map_err(|e| (n, format!("handler_map get: {e}")))?;
    if let Some(handler_id) = stored.as_deref() {
        let msg_cmd = format!(
            r#"agents message {handler_id} --simple "There are {n} new tweets in the queue.""#
        );
        let r = dispatch::run(&format!("{key_prefix}::message"), &msg_cmd)
            .await
            .map_err(|e| (n, format!("dispatch message: {e}")))?;
        if r.success() {
            return Ok(n);
        }

        // Step 2: list active and check if the handler is still alive.
        let list_r = dispatch::run(&format!("{key_prefix}::list-active"), "agents list active")
            .await
            .map_err(|e| (n, format!("dispatch list-active: {e}")))?;
        let active = list_r.active_agent_ids();
        if active.iter().any(|a| a == handler_id) {
            return Err((n, "agents message failed and handler still active".to_string()));
        }
    }

    // Step 3: spawn a fresh handler and persist its id.
    let spawn_cmd =
        format!(r#"agents spawn --simple "There are {n} new tweets in the queue.\n\nHandle each of them.""#);
    let r = dispatch::run(&format!("{key_prefix}::spawn"), &spawn_cmd)
        .await
        .map_err(|e| (n, format!("dispatch spawn: {e}")))?;
    if !r.success() {
        return Err((n, format!("agents spawn exited {}", r.exit_code)));
    }
    let spawned_id = r
        .spawned_agent_id()
        .ok_or_else(|| (n, "spawn returned no agent_id".to_string()))?;
    q.set_handler(operator, agent, &spawned_id)
        .await
        .map_err(|e| (n, format!("handler_map set: {e}")))?;
    Ok(n)
}

fn emit_per_agent(agent: &str, res: Result<usize, (usize, String)>) {
    match res {
        Ok(0) => {} // silent skip — no entries, no notification
        Ok(n) => {
            crate::emit::emit_notification(serde_json::json!({
                "event":    "agent_queue_handled",
                "agent":    agent,
                "handling": n,
                "error":    serde_json::Value::Null,
            }));
        }
        Err((n, msg)) => {
            crate::emit::emit_error(
                objectiveai_sdk::cli::output::Level::Warn,
                /* fatal */ false,
                serde_json::Value::String(format!("agent {agent}: {msg}")),
            );
            crate::emit::emit_notification(serde_json::json!({
                "event":    "agent_queue_handled",
                "agent":    agent,
                "handling": n,
                "error":    msg,
            }));
        }
    }
}
