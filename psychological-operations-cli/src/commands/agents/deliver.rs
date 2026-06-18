//! `agents deliver` — drive the browser to fulfill the reply/quote queue.
//!
//! Reads every pending `reply_quote_queue` entry, hands the batch to the
//! browser as an inline `--deliver <json>` invocation, and removes each
//! row as the browser streams back its `delivered` confirmation. Waits for
//! the browser to self-exit (no `Shutdown` sent — it exits on its own).
//!
//! Stage 1: the browser handler is a stub that exits without delivering,
//! so today this spawns, gets immediate EOF, removes nothing, and returns
//! `ok`. The CLI plumbing + wire types are in place for the real handler.

use std::io::{BufRead, BufReader};

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

    let items: Vec<DeliverItem> = entries
        .into_iter()
        .map(|e| DeliverItem {
            tweet_id: e.target_tweet_id,
            agent: e.agent_tag,
            content: e.text,
            kind: e.kind,
        })
        .collect();
    let json = serde_json::to_string(&items)
        .map_err(|e| Error::Other(format!("serialize deliver items: {e}")))?;

    let state_dir = ctx.config.state_dir();
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::Deliver { json },
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ true,
    )?;

    OutputResult::from(Event::BrowserSpawned {
        kind: "deliver".into(),
        name: None,
        pid: child.id(),
    })
    .emit();

    // Stream the browser's confirmations, removing each delivered row as it
    // lands. Loop to EOF — the browser self-exits when the batch is done.
    let child_stdout = child.stdout.take().expect("piped");
    let reader = BufReader::new(child_stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
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
        .map_err(|e| Error::Other(format!("waiting for browser failed: {e}")))?;
    OutputResult::from(Event::BrowserExit {
        kind: "deliver".into(),
        name: None,
        status: status.code(),
    })
    .emit();

    Ok(CliOutput::Ok)
}
