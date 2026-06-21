//! `agents deliver` — drive the browser to fulfill the reply/quote queue.
//!
//! Reads every pending `reply_quote_queue` entry, groups them by agent, and
//! spawns **one browser process per agent** (sequentially) with that
//! agent's items as an inline `--deliver <json>` invocation, removing each
//! row as the browser streams back its `delivered` confirmation. One
//! process per agent keeps each invocation on the proven single-agent path
//! — a single process can't reliably create a second CEF browser after
//! closing the first, so juggling all agents in one process drops everyone
//! after the first.

use std::collections::BTreeMap;

use tokio::io::{AsyncBufReadExt, BufReader};

use psychological_operations_sdk::browser::deliver::DeliverItem;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{browser_binary, launch};
use crate::error::Error;
use crate::events::Event;
use crate::output::{OutputResult, emit_result};

pub async fn run(ctx: &crate::context::Context) -> bool {
    emit_result(run_inner(ctx).await)
}

async fn run_inner(ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Delivery drives a real browser; nothing to do in mock mode.
    if ctx.config.mock {
        return Err(Error::Other(
            "deliver is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let entries = ctx
        .db
        .reply_quote_list()
        .await
        .map_err(|e| Error::Other(format!("reply_quote_list: {e}")))?;
    if entries.is_empty() {
        // Nothing pending — no point spawning a browser.
        return Ok(CliOutput::Ok);
    }

    // Group by agent (BTreeMap → stable, sorted). One browser process per
    // agent: each runs the proven single-agent path.
    let mut by_agent: BTreeMap<String, Vec<DeliverItem>> = BTreeMap::new();
    for e in entries {
        by_agent.entry(e.agent_tag.clone()).or_default().push(DeliverItem {
            tweet_id: e.target_tweet_id,
            agent: e.agent_tag,
            content: e.text,
            kind: e.kind,
        });
    }

    let state_dir = ctx.config.state_dir();
    for (agent, items) in by_agent {
        deliver_agent(ctx, &state_dir, &agent, &items).await?;
    }

    Ok(CliOutput::Ok)
}

/// Spawn one browser scoped to `agent`'s items, stream its confirmations,
/// and remove each delivered row. Returns when the browser self-exits.
async fn deliver_agent(
    ctx: &crate::context::Context,
    state_dir: &std::path::Path,
    agent: &str,
    items: &[DeliverItem],
) -> Result<(), Error> {
    let json = serde_json::to_string(items)
        .map_err(|e| Error::Other(format!("serialize deliver items: {e}")))?;

    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        state_dir,
        launch::Mode::Deliver { json },
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ true,
    )?;

    OutputResult::from(Event::BrowserSpawned {
        kind: "deliver".into(),
        name: Some(agent.to_string()),
        pid: child.id().unwrap_or(0),
    })
    .emit();

    // Stream this agent's confirmations, removing each delivered row as it
    // lands. Loop to EOF — the browser self-exits when its batch is done.
    let child_stdout = child.stdout.take().expect("piped");
    let mut lines = BufReader::new(child_stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(output) = serde_json::from_str::<Output>(&line) else {
            continue;
        };
        match output {
            Output::Delivered {
                tweet_id,
                agent,
                kind,
            } => {
                ctx.db
                    .reply_quote_delete(&agent, &kind, &tweet_id)
                    .await
                    .map_err(|e| Error::Other(format!("reply_quote_delete: {e}")))?;
                OutputResult::from(Event::Delivered {
                    tweet_id,
                    agent,
                    kind,
                })
                .emit();
            }
            Output::Error { error } => {
                OutputResult::from(Event::BrowserError { error }).emit();
            }
            _ => {}
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Other(format!("waiting for browser failed: {e}")))?;
    OutputResult::from(Event::BrowserExit {
        kind: "deliver".into(),
        name: Some(agent.to_string()),
        status: status.code(),
    })
    .emit();

    Ok(())
}
