//! `agents queue handle` — parallel per-agent dispatcher.
//!
//! Walks the queue, picks (or spawns) one objectiveai handler agent per
//! X-API agent, and runs all the per-agent tasks concurrently via a
//! `JoinSet`. Each task that actually handles work (queue ≥ 1) emits a
//! single notification:
//!
//! ```json
//! {"event":"agent_queue_handled","agent":"<name>","handling":<n>,"error":<null|msg>}
//! ```
//!
//! Agents with no queue items are skipped silently. Errors are emitted
//! twice: once as a regular plugin-output Error (so the standard error
//! surface shows it) AND once on the notification's `error` field.
//!
//! Per-agent handing-off:
//! 1. Look up the stored objectiveai handler id for
//!    `(caller instance hierarchy, agent)` in `handler_map`.
//! 2. If we have one, `agents message <handler>` via the SDK's
//!    plugin command executor.
//! 3. On failure, `agents list active` and see whether the stored
//!    handler is still in the list. If yes, the message failure was
//!    real — propagate. If no, fall through.
//! 4. `agents spawn` with the configured handler agent definition
//!    (`PSYCHOLOGICAL_OPERATIONS_QUEUE_HANDLER_AGENT`). The returned
//!    bare-id `Response` is persisted as the new handler.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use objectiveai_sdk::agent::InlineAgentBaseWithFallbacksOrRemoteCommitOptional;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::agents::list::active as list_active;
use objectiveai_sdk::cli::command::agents::message as agents_message;
use objectiveai_sdk::cli::command::agents::spawn as agents_spawn;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::Queue;
use tokio::task::JoinSet;

use crate::error::Error;

pub async fn run(
    agent_filter: Vec<String>,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent_filter, ctx).await)
}

async fn run_inner(
    agent_filter: Vec<String>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let instance_hierarchy = ctx.config
        .objectiveai_instance_hierarchy
        .as_deref()
        .ok_or_else(|| {
            Error::Other(
                "OBJECTIVEAI_INSTANCE_HIERARCHY not set — required for `agents queue handle`"
                    .into(),
            )
        })?
        .to_string();

    let handler_agent: InlineAgentBaseWithFallbacksOrRemoteCommitOptional = {
        let json = ctx.config.queue_handler_agent.as_deref().ok_or_else(|| {
            Error::Other(
                "PSYCHOLOGICAL_OPERATIONS_QUEUE_HANDLER_AGENT not set — required for \
                 `agents queue handle`"
                    .into(),
            )
        })?;
        serde_json::from_str(json).map_err(|e| {
            Error::Other(format!(
                "parse PSYCHOLOGICAL_OPERATIONS_QUEUE_HANDLER_AGENT: {e}"
            ))
        })?
    };

    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        256 * 1024 * 1024,
        Duration::from_secs(3600),
        ctx.config.objectiveai_base_dir(),
        AuthMode::XApp,
    );
    let q: Arc<Queue> = client
        .queue()
        .await
        .map_err(|e| Error::Other(format!("queue open: {e}")))?
        .clone();

    let mut agents = q
        .list_agents_with_entries()
        .await
        .map_err(|e| Error::Other(format!("list agents: {e}")))?;
    if !agent_filter.is_empty() {
        agents.retain(|a| agent_filter.iter().any(|f| f == a));
    }

    let executor = ctx.executor.clone();

    let mut tasks = JoinSet::new();
    for agent in agents {
        let instance_hierarchy = instance_hierarchy.clone();
        let q = q.clone();
        let handler_agent = handler_agent.clone();
        let executor = executor.clone();
        tasks.spawn(async move {
            let res =
                handle_one_agent(&instance_hierarchy, &agent, &q, &handler_agent, &executor).await;
            emit_per_agent(&agent, res);
        });
    }
    while tasks.join_next().await.is_some() {}

    Ok(Output::Empty)
}

async fn handle_one_agent(
    instance_hierarchy: &str,
    agent: &str,
    q: &Queue,
    handler_agent: &InlineAgentBaseWithFallbacksOrRemoteCommitOptional,
    executor: &PluginExecutor,
) -> Result<usize, (usize, String)> {
    let entries = q
        .list(agent)
        .await
        .map_err(|e| (0usize, format!("queue list: {e}")))?;
    if entries.is_empty() {
        return Ok(0);
    }
    let n = entries.len();

    // Step 1: try messaging the stored handler.
    let stored = q
        .get_handler(instance_hierarchy, agent)
        .await
        .map_err(|e| (n, format!("handler_map get: {e}")))?;
    if let Some(handler_id) = stored.as_deref() {
        let msg_req = agents_message::Request {
            path_type: agents_message::Path::AgentsMessage,
            parent_agent_instance_hierarchy: None,
            agent_instance: handler_id.to_string(),
            message: agents_message::RequestMessage::Simple(format!(
                "There are {n} new tweets in the queue."
            )),
            seed: None,
            jq: None,
        };
        match agents_message::execute(executor, msg_req, None).await {
            Ok(_resp) => {
                // Queued or Delivered — either is success.
                return Ok(n);
            }
            Err(_) => {
                // Step 2: check whether the handler is still alive.
                let list_req = list_active::Request {
                    path_type: list_active::Path::AgentsListActive,
                    parent_agent_instance_hierarchy: None,
                    jq: None,
                };
                let mut stream = list_active::execute(executor, list_req, None)
                    .await
                    .map_err(|e| (n, format!("list active: {e}")))?;
                let mut still_alive = false;
                while let Some(item) = stream.next().await {
                    let item = item.map_err(|e| (n, format!("list active stream: {e}")))?;
                    if item.agent_id == handler_id {
                        still_alive = true;
                        break;
                    }
                }
                if still_alive {
                    return Err((
                        n,
                        "agents message failed and handler still active".to_string(),
                    ));
                }
                // Fall through to spawn.
            }
        }
    }

    // Step 3: spawn a fresh handler and persist its id.
    let spawn_req = agents_spawn::Request {
        path_type: agents_spawn::Path::AgentsSpawn,
        prompt: agents_spawn::RequestPrompt::Simple(format!(
            "There are {n} new tweets in the queue.\n\nHandle each of them."
        )),
        agent: agents_spawn::AgentSpec::Resolved(handler_agent.clone()),
        seed: None,
        dangerous_advanced: None,
        jq: None,
    };
    let spawned_id = agents_spawn::execute(executor, spawn_req, None)
        .await
        .map_err(|e| (n, format!("spawn: {e}")))?;
    q.set_handler(instance_hierarchy, agent, &spawned_id)
        .await
        .map_err(|e| (n, format!("handler_map set: {e}")))?;
    Ok(n)
}

fn emit_per_agent(agent: &str, res: Result<usize, (usize, String)>) {
    match res {
        Ok(0) => {} // silent skip — no entries, no notification
        Ok(n) => {
            crate::output::OutputResult::Notification(serde_json::json!({
                "event":    "agent_queue_handled",
                "agent":    agent,
                "handling": n,
                "error":    serde_json::Value::Null,
            }))
            .emit();
        }
        Err((n, msg)) => {
            crate::output::OutputResult::error(
                objectiveai_sdk::cli::Level::Warn,
                /* fatal */ false,
                serde_json::Value::String(format!("agent {agent}: {msg}")),
            )
            .emit();
            crate::output::OutputResult::Notification(serde_json::json!({
                "event":    "agent_queue_handled",
                "agent":    agent,
                "handling": n,
                "error":    msg,
            }))
            .emit();
        }
    }
}
