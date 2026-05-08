//! `psyops oauth <name>` — drive the per-psyop OAuth 2.0 PKCE flow.
//!
//! Prereqs:
//!   - `x_app.json` has `client_id` + `client_secret`.
//!   - The per-psyop Chrome profile (`<psyops_dir>/<name>/chrome-profiles/<name>`)
//!     was created earlier via `psychological-operations browse --psyop <name>`
//!     and the operator manually signed into X with the target account.
//!   - The X App on console.x.com has `http://127.0.0.1/callback`
//!     (host-only, no port) registered as a Callback URL.

use std::time::Duration;

use crate::x_app::config as x_app_config;
use crate::chrome::{extract::ensure_extracted, native_host, paths::profile_dir};
use crate::error::Error;

use super::{pkce, server, tokens};

const AUTHORIZE_BASE: &str = "https://x.com/i/oauth2/authorize";
const SCOPE: &str = "tweet.read users.read like.write tweet.write offline.access";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn run(psyop_name: &str) -> Result<crate::Output, Error> {
    let x_app = x_app_config::ensure_setup()?;
    let client_id = x_app.client_id.as_ref()
        .expect("x_app::config::ensure_setup guarantees client_id");
    let client_secret = x_app.client_secret.as_ref()
        .expect("x_app::config::ensure_setup guarantees client_secret");

    // Verify the per-psyop dir exists. We don't strictly need the
    // psyop.json — only the chrome-profile does — but if the psyop
    // isn't on disk the operator's about to hit a confusing chromium
    // launch. Surface the issue early.
    let dir = crate::config::psyops_dir().join(psyop_name);
    if !dir.exists() {
        return Err(Error::Other(format!(
            "psyop directory does not exist: {} — run `psyops publish` first",
            dir.display(),
        )));
    }

    // PKCE + state.
    let pkce = pkce::generate();
    let state = pkce::random_state();

    // Bind the callback listener; OS picks a free ephemeral port.
    let (port, callback_fut) = server::bind_and_await(CALLBACK_TIMEOUT).await?;
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // Build the authorize URL.
    let authorize_url = format!(
        "{AUTHORIZE_BASE}?response_type=code\
         &client_id={client_id}\
         &redirect_uri={redirect}\
         &scope={scope}\
         &state={state}\
         &code_challenge={challenge}\
         &code_challenge_method=S256",
        client_id = urlencoding::encode(client_id),
        redirect  = urlencoding::encode(&redirect_uri),
        scope     = urlencoding::encode(SCOPE),
        state     = urlencoding::encode(&state),
        challenge = urlencoding::encode(&pkce.code_challenge),
    );

    // Spawn chromium on the per-psyop profile, landing on the
    // authorize URL. The profile must already be signed into X.
    let materialized = ensure_extracted()?;
    let profile = profile_dir(psyop_name);
    if !profile.exists() {
        return Err(Error::Other(format!(
            "per-psyop chrome profile does not exist: {} — run \
             `psychological-operations browse --psyop {}` and sign into X first",
            profile.display(), psyop_name,
        )));
    }
    native_host::install(&profile)?;
    crate::chrome::launch::spawn(
        &materialized.chrome_binary,
        &materialized.extension_dir,
        &profile,
        psyop_name,
        // commit isn't load-bearing for the OAuth flow (no native
        // messaging is invoked). Pass a placeholder that's harmless
        // if the extension does happen to call init.
        "oauth",
        &authorize_url,
    )?;

    eprintln!(
        "psyop \"{psyop_name}\": waiting for authorization callback on \
         127.0.0.1:{port} (timeout: {}s)",
        CALLBACK_TIMEOUT.as_secs(),
    );

    // Await the callback.
    let callback = callback_fut.await?;

    // Validate state.
    if callback.state.as_deref() != Some(&state) {
        return Err(Error::Other(format!(
            "oauth: state mismatch (expected {state:?}, got {:?})",
            callback.state,
        )));
    }
    if let Some(err) = callback.error {
        return Err(Error::Other(format!("oauth: authorization denied: {err}")));
    }
    let code = callback.code.ok_or_else(|| Error::Other(
        "oauth: callback missing `code` parameter".into(),
    ))?;

    // Exchange code -> tokens.
    let tokens = tokens::exchange_authorization_code(
        client_id,
        client_secret,
        &code,
        &pkce.code_verifier,
        &redirect_uri,
    ).await?;

    tokens::save(psyop_name, &tokens)?;
    eprintln!(
        "psyop \"{psyop_name}\": saved tokens (scope: {}, expires_at: {})",
        tokens.scope, tokens.expires_at.to_rfc3339(),
    );

    Ok(crate::Output::Empty)
}
