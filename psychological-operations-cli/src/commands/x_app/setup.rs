//! X-App setup flow.
//!
//! `setup [--dangerously-reset]` opens the embedded browser
//! against the master X-App profile so the operator can sign into
//! X.com, create their X App on console.x.com, and let the
//! browser's helpers capture credentials (`x_app.json`,
//! `post_create_dialog.html`, `oauth_popup.html`).
//!
//! Pre-flight: if the X-App is **already** fully set up
//! (signed in + both HTML snapshots complete), the flow refuses
//! unless `--dangerously-reset` is passed. The reset clears the
//! X-App's captured HTML + CEF profile AND every persona's stored
//! OAuth tokens (`auth_tokens` rows) — orphaned by the new X-App's
//! twid. CEF cookies for personas (under `cef-root/<kind>/<name>/`,
//! nested when the AIH contains '/') are intentionally preserved so
//! personas don't have to re-sign-in to X.com; they just re-run
//! `psyops login` / `agents login` against the new X-App.
//!
//! Same stream-and-shutdown shape as `login`: the CLI pipes
//! stdin + stdout, watches for `Output::XAppSetupSucceeded`,
//! sends `Request::Shutdown`, then waits.

use psychological_operations_db::signed_in_x_user_id;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::reset;
use psychological_operations_sdk::browser::x_app_credentials::{OAuthPopup, PostCreateDialog};
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{browser_binary, launch, stream};
use crate::error::Error;

pub async fn run(dangerously_reset: bool, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(dangerously_reset, ctx).await)
}

async fn run_inner(
    dangerously_reset: bool,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    // Setup drives the real embedded browser + cookie jar; there is
    // nothing to mock. Refuse outright in mock mode.
    if ctx.config.mock {
        return Err(Error::Other(
            "x-app setup is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();

    // === Pre-flight ===
    let already_set_up = is_fully_set_up(ctx).await?;
    if already_set_up && !dangerously_reset {
        return Err(Error::Other(
            "X-App is already signed in and fully set up — pass \
             --dangerously-reset to wipe the X-App credentials and every \
             persona's tokens and start over"
                .into(),
        ));
    }
    if dangerously_reset {
        reset::wipe_x_app(&ctx.db, &state_dir)
            .await
            .map_err(Error::Other)?;
        reset::wipe_all_persona_auth(&ctx.db)
            .await
            .map_err(Error::Other)?;
    }

    // === Spawn browser in XApp mode ===
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch::Mode::XApp,
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: "x_app_setup".into(),
        name: None,
        pid: child.id(),
    })
    .emit();

    let child_stdin = child.stdin.take().expect("piped");
    let child_stdout = child.stdout.take().expect("piped");

    let outcome = stream::watch_for_terminator(
        child_stdout,
        "browser exited without emitting a setup result",
        |output| match output {
            Output::XAppSetupSucceeded => Some(Ok(())),
            _ => None,
        },
    );

    stream::send_shutdown(child_stdin);

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("waiting for browser (x_app) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: "x_app_setup".into(),
        name: None,
        status: status.code(),
    })
    .emit();

    outcome.map(|()| CliOutput::Ok).map_err(Error::Other)
}

/// `true` iff the X-App is signed in (cookies) AND both captured HTML
/// snapshots parse to complete structs. Same condition the browser-side
/// panel uses to land on `PanelState::Hidden`.
async fn is_fully_set_up(ctx: &crate::context::Context) -> Result<bool, Error> {
    let state_dir = ctx.config.state_dir();
    let Some(x_app_twid) = signed_in_x_user_id(&state_dir, &Mode::XApp.cache_subdir())
        .await
        .map_err(|e| Error::Other(format!("x-app cookies probe: {e}")))?
    else {
        return Ok(false);
    };

    let post = PostCreateDialog::from_db(&ctx.db, &x_app_twid).await?;
    let popup = OAuthPopup::from_db(&ctx.db, &x_app_twid).await?;
    Ok(post.as_ref().is_some_and(|p| p.is_complete())
        && popup.as_ref().is_some_and(|p| p.is_complete()))
}
