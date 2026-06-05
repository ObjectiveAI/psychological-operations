//! Shared sign-in flow for `psyops login` and `agents login`.
//!
//! The two subcommands route through this one helper — they're
//! identical except for the [`PersonaKind`] they target.
//!
//! Flow:
//!
//! 1. **X-App preconditions.** The X-App account must be signed
//!    in (cookies), `x_app.json` must be complete, and BOTH the
//!    `post_create_dialog.html` + `oauth_popup.html` snapshots
//!    must parse to a complete struct. Any failure → single
//!    error pointing the operator at `x_app setup`.
//!
//! 2. **Persona preconditions.** The persona must NOT be signed
//!    in already AND must NOT already have an `auth.json` under
//!    the current X-App's twid. Either being set → error
//!    requiring `--dangerously-reset`.
//!
//! 3. **`--dangerously-reset`** wipes the persona's browser
//!    folder (auth dir + CEF profile) via
//!    [`psychological_operations_sdk::browser::reset::wipe_persona`]
//!    before proceeding.
//!
//! 4. Spawn the embedded browser in `PsyopAuthorize` /
//!    `AgentAuthorize` mode and wait for it to exit.

use std::path::Path;

use psychological_operations_sdk::browser::auth_json::{self, PersonaKind};
use psychological_operations_sdk::browser::cookies;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::reset;
use psychological_operations_sdk::browser::x_app_credentials::{OAuthPopup, PostCreateDialog};
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::x::x_app;

use crate::browser::{extract::ensure_extracted, launch, stream};
use crate::error::Error;

pub async fn run(
    kind: PersonaKind,
    name: &str,
    dangerously_reset: bool,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(kind, name, dangerously_reset, ctx).await)
}

async fn run_inner(
    kind: PersonaKind,
    name: &str,
    dangerously_reset: bool,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    let config_base_dir = ctx.config.objectiveai_base_dir();

    // === Pre-flight: X-App ===
    let x_app_twid = check_x_app(&config_base_dir).await?;

    // === Pre-flight: persona ===
    let persona_twid = cookies::signed_in_x_user_id(&config_base_dir, &cookie_mode(kind, name))
        .await
        .map_err(|e| Error::Other(format!("persona cookies probe: {e}")))?;
    let persona_has_auth = match persona_twid.as_deref() {
        Some(twid) => {
            auth_json::path_for(&config_base_dir, kind, name, twid, &x_app_twid).exists()
        }
        None => false,
    };
    let persona_signed_in = persona_twid.is_some();

    if persona_signed_in || persona_has_auth {
        if !dangerously_reset {
            return Err(Error::Other(format!(
                "{kind_label} '{name}' is already signed in or already has an auth.json \
                 for the current X-App — pass --dangerously-reset to wipe and re-login",
                kind_label = kind_label(kind),
            )));
        }
        reset::wipe_persona(&config_base_dir, kind, name)
            .map_err(|e| Error::Other(format!("wipe persona folder: {e}")))?;
    }

    // === Spawn browser in <kind>Authorize mode ===
    let materialized = ensure_extracted(&ctx.config)?;
    let launch_mode = match kind {
        PersonaKind::Psyop => launch::Mode::PsyopAuthorize { name: name.to_string() },
        PersonaKind::Agent => launch::Mode::AgentAuthorize { name: name.to_string() },
    };
    let event_kind = match kind {
        PersonaKind::Psyop => "psyop_login",
        PersonaKind::Agent => "agent_login",
    };

    // Pipe both: stdin so we can send `Request::Shutdown` after
    // the terminator lands; stdout so we can watch for
    // `AuthorizeSucceeded` / `AuthorizeFailed`.
    let mut child = launch::spawn(
        &materialized.binary,
        &config_base_dir,
        launch_mode,
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        pid: child.id(),
    })
    .emit();

    let child_stdin = child.stdin.take().expect("piped");
    let child_stdout = child.stdout.take().expect("piped");

    // Stream the browser's stdout until it emits the authorize
    // terminator. Helper forwards `Output::Error` to stderr and
    // silently drops Log / Panel / Url / SignedIn / TweetId.
    let outcome = stream::watch_for_terminator(
        child_stdout,
        "browser exited without emitting an authorize result",
        |output| match output {
            Output::AuthorizeSucceeded => Some(Ok(())),
            Output::AuthorizeFailed { error } => Some(Err(error.clone())),
            _ => None,
        },
    );

    // Send `Request::Shutdown` regardless of outcome — best-
    // effort. If the browser already died, the write fails
    // silently and the subsequent `child.wait()` reaps it.
    stream::send_shutdown(child_stdin);

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("waiting for browser ({name}) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        status: status.code(),
    })
    .emit();

    outcome
        .map(|()| CliOutput::Ok)
        .map_err(Error::Other)
}

const X_APP_NOT_READY: &str =
    "X-App account is not signed in or not set up with an X OAuth app — \
     complete `psychological-operations x_app setup` first";

/// Resolve the X-App's twid via cookies + verify all three
/// snapshots (`x_app.json`, `post_create_dialog.html`,
/// `oauth_popup.html`) are present + complete.
async fn check_x_app(config_base_dir: &Path) -> Result<String, Error> {
    let x_app_twid = cookies::signed_in_x_user_id(config_base_dir, &Mode::XApp)
        .await
        .map_err(|e| Error::Other(format!("x-app cookies probe: {e}")))?
        .ok_or_else(|| Error::Other(X_APP_NOT_READY.into()))?;

    let xa = x_app::config::load(config_base_dir)
        .map_err(|e| Error::Other(format!("load x_app.json: {e}")))?;
    if !xa.is_complete() {
        return Err(Error::Other(X_APP_NOT_READY.into()));
    }

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
    let post_ok = post.as_ref().is_some_and(|p| p.is_complete());
    let popup_ok = popup.as_ref().is_some_and(|p| p.is_complete());
    if !post_ok || !popup_ok {
        return Err(Error::Other(X_APP_NOT_READY.into()));
    }

    Ok(x_app_twid)
}

fn cookie_mode(kind: PersonaKind, name: &str) -> Mode {
    match kind {
        PersonaKind::Psyop => Mode::PsyopAuthorize { name: name.to_string() },
        PersonaKind::Agent => Mode::AgentAuthorize { name: name.to_string() },
    }
}

fn kind_label(kind: PersonaKind) -> &'static str {
    match kind {
        PersonaKind::Psyop => "psyop",
        PersonaKind::Agent => "agent",
    }
}
