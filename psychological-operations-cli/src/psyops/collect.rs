//! For-you collection phase of `psyops run`. Opens the embedded
//! browser in `PsyopRead` mode for one (psyop, commit), streams
//! `tweet_id` events from the browser's stdout, and enqueues
//! each into `for_you_queue` (deduped per (psyop, commit) by the
//! existing PRIMARY KEY). Returns when the operator closes the
//! browser window.
//!
//! Lifted from the old standalone `psyops browse` subcommand;
//! the multi-psyop iteration / list_psyops / derive_commit /
//! BrowseStarting / BrowseNoPsyops / BrowsePsyopList wrappers
//! are gone — `psyops run` handles all of that at its own
//! orchestration layer.

use std::io::{BufRead, BufReader};

use psychological_operations_sdk::browser::output::Output as BrowserOutput;

use crate::browser::{browser_binary, launch};
use crate::db::Db;
use crate::error::Error;

/// Materialize the browser, launch it in `PsyopRead` mode for
/// (psyop, commit), stream stdout for `tweet_id` events, and
/// enqueue each into `for_you_queue`. Returns when the operator
/// closes the browser window.
pub(crate) async fn collect_for_you(
    db: &Db,
    name: &str,
    ctx: &crate::context::Context,
) -> Result<(), Error> {
    let state_dir = ctx.config.state_dir();

    crate::output::OutputResult::from(crate::events::Event::BrowseBrowserMaterialized {
        path: ctx.config.bin_dir().display().to_string(),
    })
    .emit();

    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::PsyopRead {
            name: name.to_string(),
        },
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "psyop_read".into(),
        name: Some(name.to_string()),
        pid: child.id(),
    })
    .emit();

    // Stream the browser's stdout line-by-line. Each `tweet_id`
    // event lands in the for_you queue; anything else is logged
    // and dropped. Blocks until the operator closes the browser
    // window (stdout closes, we hit EOF, the loop exits).
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Other("browser stdout pipe missing".into()))?;
    let mut inserted: usize = 0;
    let mut skipped: usize = 0;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<BrowserOutput>(trimmed) {
            Ok(BrowserOutput::TweetId { id }) => match db.enqueue_for_you(&id, name).await {
                Ok(true) => inserted += 1,
                Ok(false) => skipped += 1,
                Err(_) => skipped += 1,
            },
            // Other events are informational here — `Log`,
            // `Url`, `SignedIn`, `Panel`, `Response`, `Help`,
            // `Error`. Drop them; the browser's own stderr
            // / its panel UI is already showing the operator
            // what they need.
            Ok(_) => {}
            Err(_) => {
                // Browser shouldn't be emitting non-JSON on
                // stdout, but be tolerant: skip and continue.
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("waiting for browser ({name}) failed: {e}")))?;
    crate::output::OutputResult::from(crate::events::Event::BrowseSessionEnded {
        psyop: name.to_string(),
        status: status.code(),
        inserted,
        skipped,
    })
    .emit();

    Ok(())
}
