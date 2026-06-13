//! OAuth 2.0 PKCE driver for `Mode::PsyopAuthorize`.
//!
//! Spawns a local callback server, navigates the CEF surface to
//! X's authorize URL, awaits the redirect, exchanges the code
//! for tokens, and persists them as
//! `<psyop-data-dir>/handles/<persona-twid>/auth.json`.
//!
//! Idempotent — `maybe_start_flow` is safe to call from every
//! cookies-watcher snapshot. It only fires when:
//!   - mode is `Mode::PsyopAuthorize`
//!   - persona is signed in (`Facts::auth_token` + `Facts::user_id`)
//!   - the auth.json doesn't already exist
//!   - no flow is already in flight in this process
//!
//! The OAuth helpers (PKCE pair, callback server, token POST)
//! are inlined below — they mirror
//! [`psychological_operations_sdk::x::oauth`] (the SDK's
//! ported OAuth scaffolding). The inline copy was originally
//! carved out when x-api was mid-refactor and doesn't compile
//! standalone; with x-api now folded into the SDK we could
//! re-share but defer that for a separate clean-up. The
//! on-disk `Tokens` blob itself lives in
//! [`psychological_operations_sdk::browser::auth_json`], which
//! owns the struct, the path math, and the atomic temp+rename
//! write. Cross-process serialization of auth writes is handled
//! separately by the SDK's two-tier SQLite `Locker`
//! (`Client::lock_auth`/`write_auth`), not by this module.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use psychological_operations_db::{Db, signed_in_x_user_id};
use psychological_operations_sdk::browser::auth_json::{PersonaKind, Tokens};
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::Output;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Wry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use urlencoding::encode as urlenc;

use crate::args::Args;
use crate::cef;
use crate::state;

const SCOPES: &str = concat!(
    "tweet.read tweet.write users.read ",
    "like.write follows.write ",
    "dm.read dm.write bookmark.write ",
    "offline.access",
);
const AUTHORIZE_BASE: &str = "https://x.com/i/oauth2/authorize";
const TOKEN_ENDPOINT: &str = "https://api.x.com/2/oauth2/token";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

// =================================================================
// One-shot flag — prevents kicking the flow twice for the same twid
// =================================================================
fn in_flight_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Called from `state::apply_cookie_facts` when the persona
/// signs out (auth_token Some → None). Clears the flag so a
/// fresh sign-in re-engages the flow.
pub fn clear_in_flight_on_signout() {
    if let Ok(mut s) = in_flight_slot().lock() {
        *s = None;
    }
}

// =================================================================
// Public entry point — called from cookies_watcher::apply_snapshot
// =================================================================
pub async fn maybe_start_flow(handle: &AppHandle<Wry>) {
    let (kind, persona_name) = match psychological_operations_sdk::browser::mode::get() {
        Some(Mode::PsyopAuthorize { name }) => (PersonaKind::Psyop, name),
        Some(Mode::AgentAuthorize { name }) => (PersonaKind::Agent, name),
        _ => return,
    };
    let Some(persona_twid) = state::current_user_id() else {
        return;
    };

    let state_dir = handle.state::<Args>().state_dir.clone();

    // The X-App master account's twid (read from the X-App CEF
    // profile's cookie jar) is part of the auth.json path —
    // different X-App accounts produce different auth.json files
    // under the same persona. If nobody is signed into the X-App
    // profile yet, the flow can't sensibly mint creds, so log and
    // bail.
    let x_app_twid = match signed_in_x_user_id(&state_dir, &Mode::XApp.cache_subdir()).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            let msg = "no X-App account signed in".to_string();
            let _ = Output::Log {
                message: format!("authorize: {msg}; not starting flow"),
            }
            .emit();
            let _ = Output::AuthorizeFailed { error: msg }.emit();
            return;
        }
        Err(e) => {
            let msg = format!("X-App cookies probe failed: {e}");
            let _ = Output::Log {
                message: format!("authorize: {msg}"),
            }
            .emit();
            let _ = Output::AuthorizeFailed { error: msg }.emit();
            return;
        }
    };

    let db = handle.state::<Db>();

    // Already have tokens for this (persona, X-App) pair? Then there's
    // nothing to do.
    let auth_exists = db
        .auth_get(kind.db_kind(), &persona_name, &persona_twid, &x_app_twid)
        .await
        .map(|t| t.is_some())
        .unwrap_or(false);
    if auth_exists {
        return;
    }

    // For psyops only, probe the cross-psyop conflict (the same twid
    // already owned by a different psyop). Agents skip it — the same X
    // account can authorize multiple agents (and psyops too).
    let conflict = match kind {
        PersonaKind::Psyop => db
            .auth_find_other_owner("psyop", &persona_twid, &persona_name)
            .await
            .ok()
            .flatten(),
        PersonaKind::Agent => None,
    };
    if let Some(other) = conflict {
        let msg = format!(
            "twid {persona_twid} belongs to PsyOp {other}; not starting flow"
        );
        let _ = Output::Log {
            message: format!("authorize: {msg}"),
        }
        .emit();
        let _ = Output::AuthorizeFailed { error: msg }.emit();
        return;
    }

    {
        let mut slot = match in_flight_slot().lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if slot.as_deref() == Some(persona_twid.as_str()) {
            return;
        }
        *slot = Some(persona_twid.clone());
    }

    let handle_for_task = handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) =
            run_flow(handle_for_task, kind, persona_name, persona_twid).await
        {
            let _ = Output::Log {
                message: format!("authorize: flow failed: {e}"),
            }
            .emit();
            let _ = Output::AuthorizeFailed { error: e }.emit();
            if let Ok(mut s) = in_flight_slot().lock() {
                *s = None;
            }
        }
    });
}

async fn run_flow(
    handle: AppHandle<Wry>,
    kind: PersonaKind,
    persona_name: String,
    persona_twid: String,
) -> Result<(), String> {
    let (client_id, client_secret) = read_x_app_creds(&handle).await?;
    let pkce = pkce_generate();
    let state_nonce = random_state();
    let (port, callback_fut) = bind_callback_server(CALLBACK_TIMEOUT)
        .await
        .map_err(|e| format!("bind callback server: {e}"))?;
    let redirect_uri =
        format!("http://127.0.0.1:{port}/psychological-operations/callback");
    let authorize_url = build_authorize_url(
        &client_id,
        &redirect_uri,
        &state_nonce,
        &pkce.code_challenge,
    );

    let _ = Output::Log {
        message: format!(
            "authorize: navigating to authorize URL on port {port}"
        ),
    }
    .emit();
    cef::navigate(authorize_url);

    let cb = callback_fut
        .await
        .map_err(|e| format!("await callback: {e}"))?;
    if let Some(err) = cb.error {
        return Err(format!("X returned error on callback: {err}"));
    }
    if cb.state.as_deref() != Some(state_nonce.as_str()) {
        return Err(format!(
            "callback state mismatch: expected {state_nonce:?}, got {:?}",
            cb.state
        ));
    }
    let code = cb.code.ok_or_else(|| "callback missing code".to_string())?;

    let tokens = exchange_code_for_tokens(
        &client_id,
        &client_secret,
        &code,
        &pkce.code_verifier,
        &redirect_uri,
    )
    .await
    .map_err(|e| format!("token exchange: {e}"))?;

    let args = handle.state::<Args>();
    let state_dir = args.state_dir.clone();
    let cache_max_size = args.cache_max_size;
    let cache_ttl = std::time::Duration::from_secs(args.cache_ttl);
    let db = handle.state::<Db>().inner().clone();
    // Use a persona-mode SDK Client (matching the kind being
    // authorized) to write the tokens under the advisory lock —
    // single seam for cross-process write coordination.
    let auth_mode = match kind {
        PersonaKind::Psyop =>
            psychological_operations_sdk::x::client::AuthMode::Psyop(persona_name.clone()),
        PersonaKind::Agent =>
            psychological_operations_sdk::x::client::AuthMode::Agent(persona_name.clone()),
    };
    let client = psychological_operations_sdk::x::client::Client::new(
        reqwest::Client::new(),
        false,
        cache_max_size,
        cache_ttl,
        state_dir,
        auth_mode,
        db,
    );
    // The Client derives its persona (and the `auth_tokens` row) from
    // `auth_mode` + the CEF cookies it consults under the hood — no
    // `PersonaKey` argument needed.
    let lock = client
        .lock_auth()
        .await
        .map_err(|e| format!("lock auth.json: {e}"))?;
    client
        .write_auth(lock, &tokens)
        .await
        .map_err(|e| format!("write auth.json: {e}"))?;
    let _ = Output::Log {
        message: format!(
            "authorize: wrote auth.json for {persona_twid} (expires_at={:?})",
            tokens.expires_at
        ),
    }
    .emit();
    state::recompute_and_publish(&handle);
    let _ = Output::AuthorizeSucceeded.emit();
    Ok(())
}

fn build_authorize_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    format!(
        "{AUTHORIZE_BASE}?response_type=code&client_id={cid}&redirect_uri={ruri}&scope={scope}&state={state}&code_challenge={chal}&code_challenge_method=S256",
        cid = urlenc(client_id),
        ruri = urlenc(redirect_uri),
        scope = urlenc(SCOPES),
        state = urlenc(state),
        chal = urlenc(code_challenge),
    )
}

// =================================================================
// PKCE — RFC 7636 §4.2 (verifier + S256 challenge) + state nonce
// =================================================================
struct Pkce {
    code_verifier: String,
    code_challenge: String,
}

fn pkce_generate() -> Pkce {
    let mut bytes = [0u8; 48];
    rand::rng().fill_bytes(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    Pkce { code_verifier, code_challenge }
}

fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// =================================================================
// Local callback server — RFC 8252 (bind 127.0.0.1:0 for an
// ephemeral port, single accept, hand-parse the GET request line)
// =================================================================
struct Callback {
    code: Option<String>,
    error: Option<String>,
    state: Option<String>,
}

async fn bind_callback_server(
    timeout: Duration,
) -> Result<(u16, impl std::future::Future<Output = Result<Callback, String>>), String>
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("bind 127.0.0.1:0: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();

    let fut = async move {
        let accept = async {
            let (mut socket, _peer) = listener
                .accept()
                .await
                .map_err(|e| format!("accept: {e}"))?;
            let mut buf = Vec::with_capacity(2048);
            let mut chunk = [0u8; 1024];
            loop {
                let n = socket
                    .read(&mut chunk)
                    .await
                    .map_err(|e| format!("read: {e}"))?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if buf.len() > 8192 {
                    return Err("oauth: request too large".into());
                }
            }
            let header = std::str::from_utf8(&buf)
                .map_err(|e| format!("non-UTF-8 request: {e}"))?;
            let request_line = header.lines().next().unwrap_or("");
            let mut it = request_line.split_whitespace();
            let _method = it.next();
            let target = it.next().unwrap_or("");
            let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
            let params: HashMap<String, String> = query
                .split('&')
                .filter(|p| !p.is_empty())
                .filter_map(|p| {
                    let (k, v) = p.split_once('=')?;
                    let k = urlencoding::decode(k).ok()?.into_owned();
                    let v = urlencoding::decode(v).ok()?.into_owned();
                    Some((k, v))
                })
                .collect();
            let body = if params.contains_key("code") {
                "<!doctype html><meta charset=utf-8><title>Authorization complete</title><body style='font:16px system-ui;padding:2em'><h2>Authorization complete</h2><p>You can close this tab.</p></body>"
            } else {
                "<!doctype html><meta charset=utf-8><title>Authorization failed</title><body style='font:16px system-ui;padding:2em'><h2>Authorization failed</h2><p>No authorization code in callback.</p></body>"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
            Ok::<_, String>(Callback {
                code: params.get("code").cloned(),
                error: params.get("error").cloned(),
                state: params.get("state").cloned(),
            })
        };
        match tokio::time::timeout(timeout, accept).await {
            Ok(r) => r,
            Err(_) => Err(format!("callback timeout after {timeout:?}")),
        }
    };

    Ok((port, fut))
}

// =================================================================
// Token exchange — POST to https://api.x.com/2/oauth2/token with
// Basic auth (client_id:client_secret) and the PKCE verifier.
// The on-disk `Tokens` blob lives in
// `psychological_operations_sdk::browser::auth_json::Tokens`; this
// module owns only the X-API wire shape that gets converted into
// it.
// =================================================================
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    scope: String,
    #[serde(default)]
    #[allow(dead_code)]
    token_type: Option<String>,
}

async fn exchange_code_for_tokens(
    client_id: &str,
    client_secret: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, String> {
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("{client_id}:{client_secret}"));
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("build reqwest client: {e}"))?;
    let resp = client
        .post(TOKEN_ENDPOINT)
        .header("Authorization", format!("Basic {basic}"))
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("POST {TOKEN_ENDPOINT}: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("token endpoint {status}: {text}"));
    }
    let parsed: TokenResponse = serde_json::from_str(&text)
        .map_err(|e| format!("parse token response: {e}: {text}"))?;
    let now = Utc::now();
    Ok(Tokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at: now + chrono::Duration::seconds(parsed.expires_in),
        scope: parsed.scope,
        saved_at: now,
    })
}

// =================================================================
// X-App credential lookup (from the db)
// =================================================================
async fn read_x_app_creds(
    handle: &AppHandle<Wry>,
) -> Result<(String, String), String> {
    use psychological_operations_sdk::browser::x_app_credentials::OAuthPopup;

    let state_dir = handle.state::<Args>().state_dir.clone();
    let db = handle.state::<Db>();

    // The OAuth popup snapshot is keyed by the X-App's signed-in twid.
    let x_app_twid = signed_in_x_user_id(&state_dir, &Mode::XApp.cache_subdir())
        .await
        .map_err(|e| format!("x-app cookies probe: {e}"))?
        .ok_or_else(|| "no X-App account signed in".to_string())?;

    let popup = OAuthPopup::from_db(db.inner(), &x_app_twid)
        .await
        .map_err(|e| format!("read oauth popup snapshot: {e}"))?
        .ok_or_else(|| "no oauth_popup snapshot captured for the X-App".to_string())?;
    let client_id = popup
        .client_id
        .ok_or_else(|| "oauth_popup snapshot missing client_id".to_string())?;
    let client_secret = popup
        .client_secret
        .ok_or_else(|| "oauth_popup snapshot missing client_secret".to_string())?;
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("client_id or client_secret is empty".into());
    }
    Ok((client_id, client_secret))
}

// Tokens are written via `Client::write_auth` from the SDK (into the
// `auth_tokens` table); the browser owns no token writer of its own.
