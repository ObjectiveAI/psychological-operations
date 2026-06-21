//! `agents login discord` — Discord bot-creation wizard.
//!
//! Opens the browser on the Discord developer portal (the shared `discord`
//! operator profile), guides the operator to sign in + create the bot, and
//! scrapes the bot token, which the browser stores in `discord_auth` keyed
//! by the agent tag. Mirrors the X authorize flow's
//! spawn → watch-terminator → shutdown → wait shape; the terminator is
//! [`Output::DiscordLoginSucceeded`] / [`Output::DiscordLoginFailed`].

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
    // The wizard drives the real embedded browser; nothing to mock.
    if ctx.config.mock {
        return Err(Error::Other(
            "login is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();

    if dangerously_reset {
        // Take the shared `discord` auth-lock (same key the browser holds)
        // so we never clear a token while the wizard is open, then drop this
        // agent's stored bot token. Released before we spawn our own browser.
        let lock_key = Mode::DiscordLogin {
            name: name.to_string(),
        }
        .cache_subdir();
        let claim = objectiveai_sdk::lockfile::try_acquire(
            &state_dir.join("browser").join("locks"),
            &lock_key,
            &format!("pid {} discord reset", std::process::id()),
        )
        .await
        .ok_or_else(|| {
            Error::Other("the Discord browser is open; close it before resetting".into())
        })?;
        let wiped = ctx.db.discord_auth_delete(name).await;
        let _ = claim.release();
        wiped?;
    }

    // Pipe both: stdin to send `Request::Shutdown` after the terminator,
    // stdout to watch for `DiscordLoginSucceeded` / `DiscordLoginFailed`.
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::DiscordLogin {
            name: name.to_string(),
        },
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "discord_login".into(),
        name: Some(name.to_string()),
        pid: child.id().unwrap_or(0),
    })
    .emit();

    let child_stdin = child.stdin.take().expect("piped");
    let child_stdout = child.stdout.take().expect("piped");

    let outcome = stream::watch_for_terminator(
        child_stdout,
        "browser exited without emitting a discord login result",
        |output| match output {
            Output::DiscordLoginSucceeded {
                client_id,
                public_key,
                bot_token,
            } => Some(Ok((client_id.clone(), public_key.clone(), bot_token.clone()))),
            Output::DiscordLoginFailed { error } => Some(Err(error.clone())),
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
        kind: "discord_login".into(),
        name: Some(name.to_string()),
        status: status.code(),
    })
    .emit();

    // Persist the scraped bot credentials CLI-side (the browser is DB-free).
    let (client_id, public_key, bot_token) = outcome.map_err(Error::Other)?;
    ctx.db
        .discord_auth_set_all(name, &client_id, &public_key, &bot_token)
        .await?;
    Ok(CliOutput::Ok)
}
