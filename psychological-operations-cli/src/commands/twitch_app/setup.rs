//! Twitch app setup flow.
//!
//! `setup [--dangerously-reset]` opens the embedded browser against the master
//! Twitch-app profile so the operator can sign into the Twitch dev console,
//! create/register their application, and let the overlay scrape its
//! `client_id` + `client_secret`. The operator must register the OAuth Redirect
//! URL as exactly [`REDIRECT_URI`] (the per-agent authorize flow binds that
//! fixed loopback port). Same stream-and-shutdown shape as `agents login x`:
//! the CLI pipes stdin + stdout, watches for `Output::TwitchAppSetupSucceeded`,
//! sends `Request::Shutdown`, then persists the scraped creds.

use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{browser_binary, launch, stream};
use crate::error::Error;

/// The fixed OAuth redirect URL the per-agent authorize flow binds (see the
/// browser's `twitch_authorize` callback port). The operator must register
/// EXACTLY this on the Twitch app — Twitch requires an exact redirect match.
pub const REDIRECT_URI: &str = "http://localhost:17563/psychological-operations/callback";

pub async fn run(dangerously_reset: bool, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(dangerously_reset, ctx).await)
}

async fn run_inner(
    dangerously_reset: bool,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    // The wizard drives the real embedded browser; nothing to mock.
    if ctx.config.mock {
        return Err(Error::Other(
            "twitch-app setup is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();

    if dangerously_reset {
        // Take the twitch-app auth-lock (same key the browser holds) so we
        // never clear creds while the wizard is open, then drop them.
        let lock_key = Mode::TwitchApp.cache_subdir();
        let claim = objectiveai_sdk::lockfile::try_acquire(
            &state_dir.join("browser").join("locks"),
            &lock_key,
            &format!("pid {} twitch-app reset", std::process::id()),
        )
        .await
        .ok_or_else(|| {
            Error::Other("the Twitch browser is open; close it before resetting".into())
        })?;
        let wiped = ctx.db.twitch_app_clear().await;
        let _ = claim.release();
        wiped?;
    }

    // Pipe both: stdin to send `Request::Shutdown` after the terminator,
    // stdout to watch for `TwitchAppSetupSucceeded` / `TwitchAppSetupFailed`.
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::TwitchApp,
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "twitch_app_setup".into(),
        name: None,
        pid: child.id().unwrap_or(0),
    })
    .emit();

    let child_stdin = child.stdin.take().expect("piped");
    let child_stdout = child.stdout.take().expect("piped");

    let outcome = stream::watch_for_terminator(
        child_stdout,
        "browser exited without emitting a twitch app setup result",
        |output| match output {
            Output::TwitchAppSetupSucceeded {
                client_id,
                client_secret,
            } => Some(Ok((client_id.clone(), client_secret.clone()))),
            Output::TwitchAppSetupFailed { error } => Some(Err(error.clone())),
            _ => None,
        },
    )
    .await;

    stream::send_shutdown(child_stdin).await;

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Other(format!("waiting for browser (twitch_app) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: "twitch_app_setup".into(),
        name: None,
        status: status.code(),
    })
    .emit();

    // Persist the scraped app credentials CLI-side (the browser is DB-free).
    let (client_id, client_secret) = outcome.map_err(Error::Other)?;
    ctx.db
        .twitch_app_set(&client_id, Some(&client_secret), Some(REDIRECT_URI))
        .await?;
    Ok(CliOutput::Ok)
}
