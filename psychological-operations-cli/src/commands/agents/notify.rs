//! `agents notify` — for every agent with queued tweets, enqueue an
//! `agents message` (keyed `psychological-operations`) telling it how
//! many tweets are waiting.
//!
//! The queue is read once for the per-`(agent, agent_kind)` counts;
//! every objectiveai `agents message` execution runs concurrently (all
//! futures pushed into a Vec and awaited together). Each message-stream
//! item or error is emitted as-is.

use futures::StreamExt;
use objectiveai_sdk::cli::command::agents::message as agents_message;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::queue::AgentKind;

use crate::error::Error;

/// Idempotency key for the per-agent notification: re-running replaces
/// the prior keyed row in objectiveai's message_queue rather than
/// stacking another.
const NOTIFY_KEY: &str = "psychological-operations";

pub async fn run(ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(ctx).await)
}

async fn run_inner(ctx: &crate::context::Context) -> Result<Output, Error> {
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
        .map_err(|e| Error::Other(format!("queue open: {e}")))?;

    // Single DB read: per-(agent, kind) tweet counts.
    let counts = q
        .counts_by_agent_kind()
        .await
        .map_err(|e| Error::Other(format!("queue counts: {e}")))?;

    // One future per agent — all concurrent. Each emits its stream
    // items / error as-is; a failed agent doesn't fail the command.
    let executor = &*ctx.executor;
    let futures = counts
        .into_iter()
        .map(|(agent, kind, n)| notify_one(executor, agent, kind, n));
    futures::future::join_all(futures).await;

    Ok(Output::Ok)
}

/// Enqueue one `agents message --enqueue-with-key psychological-operations`
/// for `agent`, emitting each stream item (or the error) as-is.
async fn notify_one(executor: &PluginExecutor, agent: String, kind: AgentKind, n: i64) {
    let request = agents_message::Request {
        path_type: agents_message::Path::AgentsMessage,
        target: target_for(&agent, kind),
        message: agents_message::RequestMessage::Simple(format!(
            "There are {n} tweets in the queue"
        )),
        enqueue: Some(agents_message::EnqueueMode::Keyed {
            key: NOTIFY_KEY.to_string(),
        }),
        dangerous_advanced: None,
        jq: None,
    };
    match agents_message::execute_streaming(executor, request, None).await {
        Ok(mut stream) => {
            while let Some(item) = stream.next().await {
                match item {
                    Ok(item) => emit_item(&item),
                    Err(e) => emit_error(&agent, &e.to_string()),
                }
            }
        }
        Err(e) => emit_error(&agent, &e.to_string()),
    }
}

/// Map a queue `(agent, agent_kind)` to an `agents message` target.
/// Tags pass through verbatim; an instance hierarchy is split at the
/// last `/` into `{parent}/{leaf}` so the composed AIH equals the
/// stored one. A slashless agent has no parent — objectiveai then
/// Config-prepends it.
fn target_for(agent: &str, kind: AgentKind) -> agents_message::MessageTarget {
    match kind {
        AgentKind::AgentTag => agents_message::MessageTarget::Tag {
            agent_tag: agent.to_string(),
        },
        AgentKind::AgentInstanceHierarchy => match agent.rsplit_once('/') {
            Some((parent, leaf)) => agents_message::MessageTarget::Direct {
                parent_agent_instance_hierarchy: Some(parent.to_string()),
                agent_instance: leaf.to_string(),
            },
            None => agents_message::MessageTarget::Direct {
                parent_agent_instance_hierarchy: None,
                agent_instance: agent.to_string(),
            },
        },
    }
}

/// Emit one message-stream item verbatim as a notification line.
fn emit_item(item: &agents_message::ResponseItem) {
    let value = serde_json::to_value(item).expect("message ResponseItem serializes");
    crate::output::OutputResult::Notification(value).emit();
}

/// Emit a non-fatal error line for one agent; the command continues.
fn emit_error(agent: &str, msg: &str) {
    crate::output::OutputResult::error(
        objectiveai_sdk::cli::Level::Warn,
        /* fatal */ false,
        serde_json::Value::String(format!("agent {agent}: {msg}")),
    )
    .emit();
}
