//! `agents notify` — for every agent with queued tweets, send it an
//! `agents message` telling it how many tweets are waiting.
//!
//! The queue is read once for the per-`(agent, agent_kind)` counts;
//! every objectiveai `agents message` execution runs concurrently (all
//! futures pushed into a Vec and awaited together). Each message
//! response or error is emitted as-is.
//!
//! Note: objectiveai 2.2.0's `agents message` is a direct send/spawn —
//! it no longer supports the keyed-enqueue (idempotent-replace) mode the
//! pre-2.2.0 version did, so re-running `agents notify` sends a fresh
//! message rather than replacing a prior keyed one.

use objectiveai_sdk::cli::command::agents::message as agents_message;
use objectiveai_sdk::cli::command::agents::selector::AgentSelector;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use psychological_operations_db::AgentKind;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::Client;

use crate::error::Error;

pub async fn run(ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(ctx).await)
}

async fn run_inner(ctx: &crate::context::Context) -> Result<Output, Error> {
    let client = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        ctx.cache_max_size,
        ctx.cache_ttl,
        ctx.config.state_dir(),
        ctx.db.clone(),
    );

    // Single DB read: per-(agent, kind) tweet counts.
    let counts = client
        .db()
        .queue_counts_by_agent_kind()
        .await
        .map_err(|e| Error::Other(format!("queue counts: {e}")))?;

    // One future per agent — all concurrent. Each emits its response /
    // error as-is; a failed agent doesn't fail the command.
    let executor = &*ctx.executor;
    let futures = counts
        .into_iter()
        .map(|(agent, kind, n)| notify_one(executor, agent, kind, n));
    futures::future::join_all(futures).await;

    Ok(Output::Ok)
}

/// Send one `agents message` to `agent`, emitting the response (or the
/// error) as-is. `agents::message::execute` answers with a single
/// `Response` (it `execute_one`s under the hood) — no stream to drain.
async fn notify_one(executor: &PluginExecutor, agent: String, kind: AgentKind, n: i64) {
    let request = agents_message::Request {
        path_type: agents_message::Path::AgentsMessage,
        agent: selector_for(&agent, kind),
        // Whitespace is fine: 2.2.0's PluginExecutor carries the nested
        // command as a structured argv array (not `argv.join(" ")`), so
        // `--simple` and this string stay separate tokens and spaces /
        // quotes inside the message survive intact.
        message: agents_message::RequestMessage::Simple(format!(
            "The account \"{agent}\" has {n} tweets in the queue."
        )),
        dangerous_advanced: None,
        base: Default::default(),
    };
    match agents_message::execute(executor, request, None).await {
        Ok(response) => emit_response(&response),
        Err(e) => emit_error(&agent, &e.to_string()),
    }
}

/// Map a queue `(agent, agent_kind)` to an `agents message` selector.
/// Tags pass through verbatim; an instance hierarchy is split at the
/// last `/` into `{parent}/{leaf}` so the composed AIH equals the
/// stored one. A slashless agent has no parent — objectiveai then
/// Config-prepends it.
fn selector_for(agent: &str, kind: AgentKind) -> AgentSelector {
    match kind {
        AgentKind::AgentTag => AgentSelector::Tag {
            agent_tag: agent.to_string(),
        },
        AgentKind::AgentInstanceHierarchy => match agent.rsplit_once('/') {
            Some((parent, leaf)) => AgentSelector::Instance {
                parent_agent_instance_hierarchy: Some(parent.to_string()),
                agent_instance: leaf.to_string(),
            },
            None => AgentSelector::Instance {
                parent_agent_instance_hierarchy: None,
                agent_instance: agent.to_string(),
            },
        },
    }
}

/// Emit one `agents message` response verbatim as a notification line.
fn emit_response(response: &agents_message::Response) {
    let value = serde_json::to_value(response).expect("message Response serializes");
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
