//! `psyops oauth <name>` — drive the per-psyop OAuth 2.0 PKCE flow.
//!
//! Prereqs:
//!   - `x_app.json` has `client_id` + `client_secret`.
//!   - The per-psyop Chromium profile (`<psyops_dir>/<name>/chromium-profiles/<name>`)
//!     was created earlier via `psychological-operations browse --psyop <name>`
//!     and the operator manually signed into X with the target account.
//!   - The X App on console.x.com has
//!     `http://127.0.0.1/psychological-operations/callback`
//!     (host-only, no port) registered as a Callback URL.

use std::time::Duration;

use psychological_operations_browser_sdk::auth_json;
use psychological_operations_x_api::oauth::{pkce, server, tokens};
use psychological_operations_x_api::x_app::config as x_app_config;
use reqwest::Client as ReqwestClient;

use crate::chromium::{extract::ensure_extracted, native_host, paths::profile_dir};
use crate::error::Error;

const AUTHORIZE_BASE: &str = "https://x.com/i/oauth2/authorize";
const SCOPE: &str = "tweet.read users.read like.write tweet.write offline.access";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn run(psyop_name: &str, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
    let config_base_dir = cfg.objectiveai_base_dir();
    let x_app = x_app_config::ensure_setup(&config_base_dir)
        .map_err(|e| Error::Other(format!("x_app.json: {e}")))?;
    let client_id = x_app.client_id.as_ref()
        .expect("x_app::config::ensure_setup guarantees client_id");
    let client_secret = x_app.client_secret.as_ref()
        .expect("x_app::config::ensure_setup guarantees client_secret");

    // Verify the per-psyop dir exists. We don't strictly need the
    // psyop.json — only the Chromium profile does — but if the psyop
    // isn't on disk the operator's about to hit a confusing chromium
    // launch. Surface the issue early.
    let dir = crate::config::psyops_dir(cfg).join(psyop_name);
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
    let (port, callback_fut) = server::bind_and_await(CALLBACK_TIMEOUT)
        .await
        .map_err(|e| Error::Other(format!("oauth server bind: {e}")))?;
    let redirect_uri =
        format!("http://127.0.0.1:{port}/psychological-operations/callback");

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
    let materialized = ensure_extracted(cfg)?;
    let profile = profile_dir(psyop_name, cfg);
    if !profile.exists() {
        return Err(Error::Other(format!(
            "per-psyop Chromium profile does not exist: {} — run \
             `psychological-operations psyops browse --name {}` and sign into X first",
            profile.display(), psyop_name,
        )));
    }
    native_host::install(&profile, cfg)?;
    // Discard the Child — the OAuth dance is async (we await the
    // local callback below). Chromium stays open until the operator
    // closes it; we don't block on that.
    let _child = crate::chromium::launch::spawn(
        &materialized.chromium_binary,
        &materialized.read_extension_dir,
        &profile,
        psyop_name,
        // commit isn't load-bearing for the OAuth flow (no native
        // messaging is invoked). Pass a placeholder that's harmless
        // if the extension does happen to call init.
        "oauth",
        &authorize_url,
    )?;

    crate::emit::emit(crate::events::Event::OauthListening {
        psyop: psyop_name.to_string(),
        port,
        timeout_secs: CALLBACK_TIMEOUT.as_secs(),
    });

    // Await the callback.
    let callback = callback_fut
        .await
        .map_err(|e| Error::Other(format!("oauth callback: {e}")))?;

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
    )
    .await
    .map_err(|e| Error::Other(format!("token exchange: {e}")))?;

    // Resolve the persona's numeric user-id via /2/users/me using
    // the freshly-minted access token. SDK's `auth_json::set` is
    // keyed by twid (so the OAuth flow writes deterministically to
    // ONE persona's slot even if chromium has multiple cookie
    // profiles open) — and the X API is the canonical source for
    // "who did the user just authenticate as" without poking the
    // CLI's chromium cookie store.
    let twid = resolve_user_id(&tokens.access_token).await?;

    auth_json::set(&config_base_dir, psyop_name, &twid, &tokens)
        .await
        .map_err(|e| Error::Other(format!("auth_json::set: {e}")))?;

    crate::emit::emit(crate::events::Event::OauthTokensSaved {
        psyop: psyop_name.to_string(),
        scope: tokens.scope.clone(),
        expires_at: tokens.expires_at.to_rfc3339(),
    });

    Ok(crate::Output::Empty)
}

/// GET `/2/users/me` with `access_token` and return the persona's
/// numeric `id`. The user-context token always has `users.read`
/// scope (it's in [`SCOPE`]), so this is a safe immediate-after-
/// exchange probe.
async fn resolve_user_id(access_token: &str) -> Result<String, Error> {
    #[derive(serde::Deserialize)]
    struct Me {
        data: MeData,
    }
    #[derive(serde::Deserialize)]
    struct MeData {
        id: String,
    }

    let resp = ReqwestClient::new()
        .get("https://api.x.com/2/users/me")
        .header("authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| Error::Other(format!("/2/users/me transport: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| Error::Other(format!("/2/users/me body read: {e}")))?;
    if !status.is_success() {
        return Err(Error::Other(format!("/2/users/me {status}: {text}")));
    }
    let me: Me = serde_json::from_str(&text)
        .map_err(|e| Error::Other(format!("/2/users/me parse: {e}: {text}")))?;
    Ok(me.data.id)
}
