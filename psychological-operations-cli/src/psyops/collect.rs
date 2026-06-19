//! For-you collection phase of `psyops run`. Opens the embedded
//! browser in `AgentRead` mode for one agent, streams `tweet_id`
//! events from the browser's stdout, and returns the distinct IDs
//! (in arrival order) **in memory** — nothing is persisted. Returns
//! when the operator closes the browser window.
//!
//! Collection is per-agent (a For You feed belongs to an agent, not a
//! psyop); the caller fans the returned IDs out to every psyop in the
//! run that references this agent in its `for_you`.

use std::io::{BufRead, BufReader};

use psychological_operations_sdk::browser::output::Output as BrowserOutput;

use crate::browser::{browser_binary, launch};
use crate::error::Error;

/// Materialize the browser, launch it in `AgentRead` mode for `agent_tag`,
/// stream stdout for `tweet_id` events, and return the distinct IDs in
/// arrival order. Returns when the operator closes the browser window.
pub(crate) async fn collect_for_you(
    agent_tag: &str,
    ctx: &crate::context::Context,
) -> Result<Vec<String>, Error> {
    let state_dir = ctx.config.state_dir();

    crate::output::OutputResult::from(crate::events::Event::BrowseBrowserMaterialized {
        path: ctx.config.bin_dir().display().to_string(),
    })
    .emit();

    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::AgentRead {
            name: agent_tag.to_string(),
        },
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "agent_read".into(),
        name: Some(agent_tag.to_string()),
        pid: child.id(),
    })
    .emit();

    // Stream the browser's stdout line-by-line. Each `tweet_id` event is
    // appended in arrival order; we do NOT de-duplicate here (the browser's
    // own per-session dedup already collapses repeated HTML snapshots, so
    // each tweet is emitted once per feed). Blocks until the operator closes
    // the browser window (stdout closes, we hit EOF, the loop exits).
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Other("browser stdout pipe missing".into()))?;
    let mut ids: Vec<String> = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<BrowserOutput>(trimmed) {
            Ok(BrowserOutput::TweetId { id }) => {
                ids.push(id);
            }
            // Other events are informational here — `Log`, `Url`,
            // `SignedIn`, `Panel`, `Response`, `Help`, `Error`. Drop
            // them; the browser's own stderr / panel UI shows the
            // operator what they need.
            Ok(_) => {}
            Err(_) => {
                // Browser shouldn't emit non-JSON on stdout, but be
                // tolerant: skip and continue.
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("waiting for browser ({agent_tag}) failed: {e}")))?;
    crate::output::OutputResult::from(crate::events::Event::BrowseSessionEnded {
        agent: agent_tag.to_string(),
        status: status.code(),
        collected: ids.len(),
    })
    .emit();

    Ok(ids)
}
