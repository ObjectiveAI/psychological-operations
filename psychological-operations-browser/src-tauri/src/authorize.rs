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
//! owns the struct, the path math, the fs4 advisory-lock
//! pattern, and the atomic temp+rename write.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use psychological_operations_sdk::browser::auth_json::{self, PersonaKind, Tokens};
use psychological_operations_sdk::browser::cookies::signed_in_x_user_id;
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
use crate::webview;

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

    let config_base_dir = handle.state::<Args>().config_base_dir.clone();

    // The X-App master account's twid (read from the X-App CEF
    // profile's cookie jar) is part of the auth.json path —
    // different X-App accounts produce different auth.json files
    // under the same persona. If nobody is signed into the X-App
    // profile yet, the flow can't sensibly mint creds, so log and
    // bail.
    let x_app_twid = {
        let base = config_base_dir.clone();
        match tokio::task::spawn_blocking(move || signed_in_x_user_id(&base, &Mode::XApp))
            .await
        {
            Ok(Ok(Some(t))) => t,
            Ok(Ok(None)) => {
                let _ = Output::Log {
                    message: "authorize: no X-App account signed in; not starting flow".into(),
                }
                .emit();
                return;
            }
            Ok(Err(e)) => {
                let _ = Output::Log {
                    message: format!("authorize: X-App cookies probe failed: {e}"),
                }
                .emit();
                return;
            }
            Err(e) => {
                let _ = Output::Log {
                    message: format!("authorize: X-App cookies join failed: {e}"),
                }
                .emit();
                return;
            }
        }
    };

    let auth_path = auth_json::path_for(
        &config_base_dir,
        kind,
        &persona_name,
        &persona_twid,
        &x_app_twid,
    );

    // Concurrent: check whether we already have auth.json AND
    // (for psyops only) probe the cross-psyop conflict. Both
    // disk-bound + independent — `tokio::join!` halves the
    // latency on every cookies kick where either probe is
    // non-trivial. Agents skip the conflict scan because the
    // same X account can be signed into multiple agents (and
    // psyops too) without complaint.
    let conflict_fut = async {
        match kind {
            PersonaKind::Psyop => {
                find_other_psyop_owning_twid(handle, &persona_name, &persona_twid)
                    .await
            }
            PersonaKind::Agent => None,
        }
    };
    let (auth_exists, conflict) = tokio::join!(
        async { tokio::fs::try_exists(&auth_path).await.unwrap_or(false) },
        conflict_fut,
    );
    if auth_exists {
        return;
    }
    if let Some(other) = conflict {
        let _ = Output::Log {
            message: format!(
                "authorize: twid {persona_twid} belongs to PsyOp {other}; not starting flow"
            ),
        }
        .emit();
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
            run_flow(handle_for_task, kind, persona_name, persona_twid, x_app_twid).await
        {
            let _ = Output::Log {
                message: format!("authorize: flow failed: {e}"),
            }
            .emit();
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
    x_app_twid: String,
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

    let config_base_dir = handle.state::<Args>().config_base_dir.clone();
    auth_json::set(
        &config_base_dir,
        kind,
        &persona_name,
        &persona_twid,
        &x_app_twid,
        &tokens,
    )
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
// Disk helpers
// =================================================================
async fn read_x_app_creds(
    handle: &AppHandle<Wry>,
) -> Result<(String, String), String> {
    use psychological_operations_sdk::browser::x_app_credentials::OAuthPopup;

    let handles_dir = webview::mode_data_dir(handle, &Mode::XApp).join("handles");

    // Async directory walk: gather every subdir under handles/.
    let mut rd = tokio::fs::read_dir(&handles_dir)
        .await
        .map_err(|e| format!("read {}: {e}", handles_dir.display()))?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    while let Ok(Some(ent)) = rd.next_entry().await {
        let path = ent.path();
        if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) {
            dirs.push(path);
        }
    }
    if dirs.is_empty() {
        return Err(format!("no X-App handles under {}", handles_dir.display()));
    }

    // Fan out per-dir mtime fetches concurrently — each
    // `metadata().modified()` is an independent stat.
    let mtimes = futures::future::join_all(dirs.iter().map(|dir| {
        let snap = dir.join(crate::credentials::OAUTH_POPUP_FILE);
        async move {
            tokio::fs::metadata(&snap)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
        }
    }))
    .await;

    let mut owner: Vec<_> = dirs
        .into_iter()
        .zip(mtimes)
        .filter_map(|(dir, mt)| mt.map(|t| (t, dir)))
        .collect();
    owner.sort_by_key(|(t, _)| *t);
    let (_, dir) = owner.pop().ok_or_else(|| {
        format!(
            "no {} under any X-App handle dir in {}",
            crate::credentials::OAUTH_POPUP_FILE,
            handles_dir.display()
        )
    })?;
    let popup_path = dir.join(crate::credentials::OAUTH_POPUP_FILE);
    let popup = OAuthPopup::load(&popup_path)
        .await
        .map_err(|e| format!("read {}: {e}", popup_path.display()))?
        .ok_or_else(|| {
            format!("{} disappeared mid-read", popup_path.display())
        })?;
    let client_id = popup
        .client_id
        .ok_or_else(|| format!("{} missing client_id", popup_path.display()))?;
    let client_secret = popup.client_secret.ok_or_else(|| {
        format!("{} missing client_secret", popup_path.display())
    })?;
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("client_id or client_secret is empty".into());
    }
    Ok((client_id, client_secret))
}

/// Scan every sibling psyop's data dir and return the name of
/// the first one (other than `current`) that holds ANY
/// `handles/<twid>/<x_app_twid>/auth.json` (for any X-App twid
/// leaf). Sorting psyop entries alphabetically gives
/// deterministic conflict reporting if multiple owners exist;
/// per-sibling and per-x_app_twid presence checks fan out
/// concurrently via [`futures::future::join_all`].
///
/// Returns `None` when no conflict, `twid` is empty, or the
/// `psyop/` root doesn't exist yet (first-ever run).
pub async fn find_other_psyop_owning_twid(
    handle: &AppHandle<Wry>,
    current: &str,
    twid: &str,
) -> Option<String> {
    if twid.is_empty() {
        return None;
    }
    // Walk up one level from this psyop's own data dir to reach
    // the `psyop/` root. Both Read and Authorize resolve to
    // the same dir, so either works here.
    let current_mode = Mode::PsyopAuthorize {
        name: current.to_string(),
    };
    let psyop_root = webview::mode_data_dir(handle, &current_mode)
        .parent()?
        .to_path_buf();
    let config_base_dir = handle.state::<Args>().config_base_dir.clone();

    let mut rd = tokio::fs::read_dir(&psyop_root).await.ok()?;
    let mut entries: Vec<PathBuf> = Vec::new();
    while let Ok(Some(ent)) = rd.next_entry().await {
        let path = ent.path();
        if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) {
            entries.push(path);
        }
    }
    entries.sort();

    let checks = entries.iter().map(|path| {
        let sibling_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let config_base_dir = config_base_dir.clone();
        let twid = twid.to_string();
        async move {
            let Some(name) = sibling_name else { return false };
            // <sibling>/handles/<twid>/ — list each x_app_twid leaf
            // dir and check it for auth.json.
            let persona_dir = config_base_dir
                .join("plugins")
                .join("psychological-operations")
                .join("browser")
                .join("psyop")
                .join(&name)
                .join("handles")
                .join(&twid);
            let mut rd = match tokio::fs::read_dir(&persona_dir).await {
                Ok(r) => r,
                Err(_) => return false,
            };
            let mut candidates: Vec<String> = Vec::new();
            while let Ok(Some(ent)) = rd.next_entry().await {
                if tokio::fs::metadata(&ent.path())
                    .await
                    .is_ok_and(|m| m.is_dir())
                {
                    if let Some(s) = ent.file_name().to_str() {
                        candidates.push(s.to_string());
                    }
                }
            }
            let presence = futures::future::join_all(candidates.iter().map(|x_app_twid| {
                let p = auth_json::path_for(
                    &config_base_dir,
                    PersonaKind::Psyop,
                    &name,
                    &twid,
                    x_app_twid,
                );
                async move { tokio::fs::try_exists(&p).await.unwrap_or(false) }
            }))
            .await;
            presence.into_iter().any(|x| x)
        }
    });
    let existence = futures::future::join_all(checks).await;

    for (path, exists) in entries.iter().zip(existence) {
        if !exists {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == current {
            continue;
        }
        return Some(name.to_string());
    }
    None
}

// auth.json is written via `auth_json::set` from the SDK; the
// browser owns no on-disk `Tokens` writer of its own.
