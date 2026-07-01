//! `agents login twitch` — per-agent Twitch OAuth authorize.
//!
//! Reads the master Twitch app's `client_id` + `client_secret` (from
//! `twitch-app setup`), opens the browser under the agent's CEF profile, and
//! drives the OAuth code flow. The browser signs the operator into the agent's
//! Twitch account (Twitch's own login+consent), exchanges the code, validates
//! for the account's `user_id`/`login`, and emits the tokens; the CLI persists
//! them to `twitch_auth`. Mirrors `agents login discord`; the terminator is
//! [`Output::TwitchAuthorizeSucceeded`] / [`Output::TwitchAuthorizeFailed`].

use psychological_operations_db::TwitchAuth;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{browser_binary, launch, stream};
use crate::error::Error;

pub async fn run(name: &str, dangerously_reset: bool, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(name, dangerously_reset, ctx).await)
}

async fn run_inner(
    name: &str,
    dangerously_reset: bool,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    // The authorize flow drives the real embedded browser; nothing to mock.
    if ctx.config.mock {
        return Err(Error::Other(
            "login is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();

    // The master Twitch app's OAuth client creds drive the token exchange.
    let app = ctx.db.twitch_app_active().await?.ok_or_else(|| {
        Error::Other("no Twitch app is set up — run `twitch-app setup` first".into())
    })?;
    let client_id = app.client_id;
    let client_secret = app.client_secret.ok_or_else(|| {
        Error::Other("the Twitch app has no client_secret — re-run `twitch-app setup`".into())
    })?;

    if dangerously_reset {
        // Take the agent's browser auth-lock (shared with its X profile) so we
        // never clear tokens while a browser is open, then drop the agent's
        // stored Twitch token. Released before we spawn our own browser.
        let lock_key = Mode::TwitchAuthorize {
            name: name.to_string(),
        }
        .cache_subdir();
        let claim = objectiveai_sdk::lockfile::try_acquire(
            &state_dir.join("browser").join("locks"),
            &lock_key,
            &format!("pid {} twitch reset", std::process::id()),
        )
        .await
        .ok_or_else(|| {
            Error::Other("the agent's browser is open; close it before resetting".into())
        })?;
        let wiped = ctx.db.twitch_auth_delete(name).await;
        let _ = claim.release();
        wiped?;
    }

    // Pipe both: stdin to send `Request::Shutdown` after the terminator,
    // stdout to watch for `TwitchAuthorizeSucceeded` / `TwitchAuthorizeFailed`.
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::TwitchAuthorize {
            name: name.to_string(),
            client_id,
            client_secret,
        },
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "twitch_login".into(),
        name: Some(name.to_string()),
        pid: child.id().unwrap_or(0),
    })
    .emit();

    let child_stdin = child.stdin.take().expect("piped");
    let child_stdout = child.stdout.take().expect("piped");

    let outcome = stream::watch_for_terminator(
        child_stdout,
        "browser exited without emitting a twitch login result",
        |output| match output {
            Output::TwitchAuthorizeSucceeded {
                user_id,
                login,
                tokens,
            } => Some(Ok((user_id.clone(), login.clone(), tokens.clone()))),
            Output::TwitchAuthorizeFailed { error } => Some(Err(error.clone())),
            _ => None,
        },
    )
    .await;

    stream::send_shutdown(child_stdin).await;

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Other(format!("waiting for browser ({name}) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: "twitch_login".into(),
        name: Some(name.to_string()),
        status: status.code(),
    })
    .emit();

    // Persist the minted tokens CLI-side (the browser is DB-free). A running
    // daemon picks up the new auth via the twitch_auth NOTIFY trigger.
    let (user_id, login, tokens) = outcome.map_err(Error::Other)?;
    ctx.db
        .twitch_auth_set(
            name,
            &TwitchAuth {
                user_id: Some(user_id),
                login: Some(login),
                access_token: Some(tokens.access_token),
                refresh_token: tokens.refresh_token,
                scope: Some(tokens.scope),
                expires_at: Some(tokens.expires_at),
            },
        )
        .await?;
    Ok(CliOutput::Ok)
}
