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
//! unless `--dangerously-reset` is passed. The reset wipes the
//! X-App folder (browser data + CEF profile) AND every named
//! psyop/agent persona dir under `browser/{psyop,agent}/*/` —
//! their auth.jsons are orphaned by the new X-App's twid.
//! CEF cookies for personas (under `cef-root/<kind>-<name>/`)
//! are intentionally preserved so personas don't have to
//! re-sign-in to X.com; they just re-run `psyops login` /
//! `agents login` against the new X-App.
//!
//! Same stream-and-shutdown shape as `login`: the CLI pipes
//! stdin + stdout, watches for `Output::XAppSetupSucceeded`,
//! sends `Request::Shutdown`, then waits.

use std::path::Path;

use psychological_operations_sdk::browser::cookies;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::reset;
use psychological_operations_sdk::browser::x_app_credentials::{OAuthPopup, PostCreateDialog};
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{extract::ensure_extracted, launch, stream};
use crate::error::Error;

pub async fn run(
    dangerously_reset: bool,
    cfg: &crate::run::Config,
) -> Result<CliOutput, Error> {
    let config_base_dir = cfg.objectiveai_base_dir();

    // === Pre-flight ===
    let already_set_up = is_fully_set_up(&config_base_dir).await?;
    if already_set_up && !dangerously_reset {
        return Err(Error::Other(
            "X-App is already signed in and fully set up — pass \
             --dangerously-reset to wipe the X-App folder and every \
             persona's auth.json and start over"
                .into(),
        ));
    }
    if dangerously_reset {
        reset::wipe_x_app(&config_base_dir)
            .map_err(|e| Error::Other(format!("wipe x-app folder: {e}")))?;
        reset::wipe_all_persona_auth_dirs(&config_base_dir)
            .map_err(|e| Error::Other(format!("wipe persona auth dirs: {e}")))?;
    }

    // === Spawn browser in XApp mode ===
    let materialized = ensure_extracted(cfg)?;
    let mut child = launch::spawn(
        &materialized.binary,
        &config_base_dir,
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

    outcome
        .map(|()| CliOutput::Empty)
        .map_err(Error::Other)
}

/// `true` iff the X-App is signed in (cookies) AND both HTML
/// snapshots (`post_create_dialog.html` + `oauth_popup.html`)
/// load to complete structs. Same condition the browser-side
/// panel uses to land on `PanelState::Hidden`.
async fn is_fully_set_up(config_base_dir: &Path) -> Result<bool, Error> {
    let Some(x_app_twid) = cookies::signed_in_x_user_id(config_base_dir, &Mode::XApp)
        .await
        .map_err(|e| Error::Other(format!("x-app cookies probe: {e}")))?
    else {
        return Ok(false);
    };

    let x_app_handle_dir = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("x-app")
        .join("handles")
        .join(&x_app_twid);
    let post = PostCreateDialog::load(&x_app_handle_dir.join("post_create_dialog.html"))
        .await
        .map_err(|e| Error::Other(format!("load post_create_dialog: {e}")))?;
    let popup = OAuthPopup::load(&x_app_handle_dir.join("oauth_popup.html"))
        .await
        .map_err(|e| Error::Other(format!("load oauth_popup: {e}")))?;
    Ok(post.as_ref().is_some_and(|p| p.is_complete())
        && popup.as_ref().is_some_and(|p| p.is_complete()))
}
