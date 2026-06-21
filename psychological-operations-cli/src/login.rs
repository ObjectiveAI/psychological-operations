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
//!    error pointing the operator at `x-app setup`.
//!
//! 2. **Persona preconditions.** Refuse only if the persona already
//!    has stored tokens for the current X-App's twid. Being merely
//!    signed in to X.com (persistent cookies) is the normal
//!    pre-consent state and proceeds — that's when the authorize flow
//!    detects the sign-in and fires. `--dangerously-reset` wipes the
//!    persona (tokens + CEF profile) and re-logs in regardless.
//!
//! 3. **`--dangerously-reset`** wipes the persona's browser
//!    folder (auth dir + CEF profile) via
//!    [`psychological_operations_sdk::browser::reset::wipe_persona`]
//!    before proceeding.
//!
//! 4. Spawn the embedded browser in `PsyopAuthorize` /
//!    `AgentAuthorize` mode and wait for it to exit.

use psychological_operations_sdk::browser::auth_json::PersonaKind;
use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::reset;
use psychological_operations_sdk::browser::x_app_credentials::{OAuthPopup, PostCreateDialog};
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::browser::{browser_binary, launch, stream};
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
    // Login drives the real embedded browser + cookie jar; there is
    // nothing to mock. Refuse outright in mock mode.
    if ctx.config.mock {
        return Err(Error::Other(
            "login is not supported in mock mode (PSYCHOLOGICAL_OPERATIONS_MOCK)".into(),
        ));
    }

    let state_dir = ctx.config.state_dir();

    // === Pre-flight: X-App must be set up; read its OAuth client creds +
    // active twid so we can hand them to the (DB-free) browser. ===
    let (client_id, client_secret, x_app_twid) = read_x_app_creds(ctx).await?;

    // `--dangerously-reset` forgets the persona's account mapping + wipes
    // its CEF profile for a clean re-login (e.g. to switch to a different X
    // account). Otherwise login always opens the browser: once the operator
    // signs in, the authorize flow establishes the persona→twid mapping and
    // either mints a token or — if that account already has one — skips the
    // OAuth consent and closes.
    if dangerously_reset {
        // Take the same auth lock the browser holds for this identity so we
        // never wipe an account a running browser is using. Key derived from
        // `kind.to_mode(name).cache_subdir()` (= `agent-<tag>`) so both sides
        // agree, including the `/`→`-` flattening for hierarchy tags.
        let lock_key = kind.to_mode(name).cache_subdir();
        let claim = objectiveai_sdk::lockfile::try_acquire(
            &state_dir.join("browser").join("locks"),
            &lock_key,
            &format!("pid {} persona reset", std::process::id()),
        )
        .await
        .ok_or_else(|| {
            Error::Other(format!(
                "auth for '{name}' is locked by a running browser; close it before resetting"
            ))
        })?;
        let wiped = reset::wipe_persona(&ctx.db, &state_dir, kind, name).await;
        // Explicitly release now (drop is a no-op) — before this command
        // spawns its own browser below, which re-acquires the same lock.
        let _ = claim.release();
        wiped.map_err(Error::Other)?;
    } else if let Some(twid) = ctx.db.persona_twid_get(kind.db_kind(), name).await? {
        // Already logged in — the persona maps to an account that already has
        // tokens. Nothing to mint; short-circuit. (`--dangerously-reset` to
        // switch accounts.)
        if ctx.db.account_auth_get(&twid).await?.is_some() {
            return Ok(CliOutput::Ok);
        }
    }

    // === Spawn browser in <kind>Authorize mode ===
    let launch_mode = match kind {
        PersonaKind::Agent => launch::Mode::AgentAuthorize {
            name: name.to_string(),
            client_id,
            client_secret,
        },
    };
    let event_kind = match kind {
        PersonaKind::Agent => "agent_login",
    };

    // Pipe both: stdin so we can send `Request::Shutdown` after
    // the terminator lands; stdout so we can watch for
    // `AuthorizeSucceeded` / `AuthorizeFailed`.
    let mut child = launch::spawn(
        &browser_binary(&ctx.config),
        &state_dir,
        launch_mode,
        /* pipe_stdin  = */ true,
        /* pipe_stdout = */ true,
    )?;

    crate::output::OutputResult::from(crate::events::Event::BrowserSpawned {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        pid: child.id().unwrap_or(0),
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
            Output::AuthorizeSucceeded {
                persona_twid,
                tokens,
            } => Some(Ok((persona_twid.clone(), tokens.clone()))),
            Output::AuthorizeFailed { error } => Some(Err(error.clone())),
            _ => None,
        },
    )
    .await;

    // Send `Request::Shutdown` regardless of outcome — best-
    // effort. If the browser already died, the write fails
    // silently and the subsequent `child.wait()` reaps it.
    stream::send_shutdown(child_stdin).await;

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Other(format!("waiting for browser ({name}) failed: {e}")))?;

    crate::output::OutputResult::from(crate::events::Event::BrowserExit {
        kind: event_kind.into(),
        name: Some(name.to_string()),
        status: status.code(),
    })
    .emit();

    // Persist the minted tokens CLI-side (the browser is DB-free). The
    // persona→twid mapping + the account's token row both key off the
    // emitted persona_twid; `x_app_twid` rides along for refresh/provenance.
    let (persona_twid, tokens) = outcome.map_err(Error::Other)?;
    ctx.db
        .persona_twid_set(kind.db_kind(), name, &persona_twid)
        .await?;
    let tokens_value =
        serde_json::to_value(&tokens).map_err(|e| Error::Other(format!("serialize tokens: {e}")))?;
    ctx.db
        .account_auth_set(&persona_twid, &x_app_twid, &tokens_value)
        .await?;
    Ok(CliOutput::Ok)
}

const X_APP_NOT_READY: &str = "X-App account is not set up with an X OAuth app — \
     complete `psychological-operations x-app setup` first";

/// Verify an X-App is set up and return its OAuth client creds + active twid.
/// Requires the active twid (from `x_app_html`) to resolve and both captured
/// HTML snapshots to be present + complete. Reads the DB, not cookies.
async fn read_x_app_creds(
    ctx: &crate::context::Context,
) -> Result<(String, String, String), Error> {
    let x_app_twid = ctx
        .db
        .x_app_twid_active()
        .await?
        .ok_or_else(|| Error::Other(X_APP_NOT_READY.into()))?;

    let post = PostCreateDialog::from_db(&ctx.db, &x_app_twid).await?;
    let popup = OAuthPopup::from_db(&ctx.db, &x_app_twid).await?;
    let post_ok = post.as_ref().is_some_and(|p| p.is_complete());
    let popup_ok = popup.as_ref().is_some_and(|p| p.is_complete());
    if !post_ok || !popup_ok {
        return Err(Error::Other(X_APP_NOT_READY.into()));
    }
    // `is_complete()` guarantees both fields are `Some`.
    let popup = popup.expect("popup present");
    Ok((
        popup.client_id.expect("client_id present"),
        popup.client_secret.expect("client_secret present"),
        x_app_twid,
    ))
}
