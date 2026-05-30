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
//! The OAuth helpers (PKCE pair, callback server, token POST,
//! `Tokens` struct + JSON shape) are inlined below. They mirror
//! what `psychological-operations-x-api/src/oauth/` does — that
//! crate is half-refactored and doesn't compile standalone, so
//! pulling it in as a dep wasn't an option.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use psychological_operations_browser_sdk::mode::Mode;
use psychological_operations_browser_sdk::output::Output;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Wry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use urlencoding::encode as urlenc;

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
pub fn maybe_start_flow(handle: &AppHandle<Wry>) {
    let psyop_name = match psychological_operations_browser_sdk::mode::get() {
        Some(Mode::PsyopAuthorize { name }) => name,
        _ => return,
    };
    let Some(persona_twid) = state::current_user_id() else {
        return;
    };

    let auth_path = auth_json_path(handle, &psyop_name, &persona_twid);
    if auth_path.exists() {
        return;
    }

    // Cross-psyop guard: if this twid already belongs to a
    // different psyop, don't kick the dance — we'd be minting
    // tokens onto the wrong handles/. The panel surfaces the
    // "wrong account" nag separately; this is just the OAuth-
    // side short-circuit. Re-evaluated on every cookies kick,
    // so it auto-clears when the user signs back in correctly.
    if let Some(other) = find_other_psyop_owning_twid(handle, &psyop_name, &persona_twid) {
        let _ = Output::Log {
            message: format!(
                "psyop_authorize: twid {persona_twid} belongs to PsyOp {other}; not starting flow"
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
        if let Err(e) = run_flow(handle_for_task, psyop_name, persona_twid).await {
            let _ = Output::Log {
                message: format!("psyop_authorize: flow failed: {e}"),
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
    psyop_name: String,
    persona_twid: String,
) -> Result<(), String> {
    let (client_id, client_secret) = read_x_app_creds(&handle)?;
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
            "psyop_authorize: navigating to authorize URL on port {port}"
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

    write_auth_json(&handle, &psyop_name, &persona_twid, &tokens)?;
    let _ = Output::Log {
        message: format!(
            "psyop_authorize: wrote auth.json for {persona_twid} (expires_at={:?})",
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
// =================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub scope: String,
    pub saved_at: DateTime<Utc>,
}

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
fn read_x_app_creds(handle: &AppHandle<Wry>) -> Result<(String, String), String> {
    let handles_dir = webview::mode_data_dir(handle, &Mode::XApp).join("handles");
    let entries: Vec<_> = fs::read_dir(&handles_dir)
        .map_err(|e| format!("read {}: {e}", handles_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    if entries.is_empty() {
        return Err(format!("no X-App handles under {}", handles_dir.display()));
    }
    let mut owner = entries
        .into_iter()
        .filter_map(|e| {
            let cid = e.path().join("client_id.txt");
            let mtime = cid.metadata().ok().and_then(|m| m.modified().ok())?;
            Some((mtime, e.path()))
        })
        .collect::<Vec<_>>();
    owner.sort_by_key(|(t, _)| *t);
    let (_, dir) = owner.pop().ok_or_else(|| {
        format!(
            "no client_id.txt under any X-App handle dir in {}",
            handles_dir.display()
        )
    })?;
    let client_id = fs::read_to_string(dir.join("client_id.txt"))
        .map_err(|e| format!("read client_id.txt: {e}"))?
        .trim()
        .to_string();
    let client_secret = fs::read_to_string(dir.join("client_secret.txt"))
        .map_err(|e| format!("read client_secret.txt: {e}"))?
        .trim()
        .to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("client_id or client_secret is empty".into());
    }
    Ok((client_id, client_secret))
}

fn auth_json_path(handle: &AppHandle<Wry>, psyop_name: &str, twid: &str) -> PathBuf {
    let mode = Mode::PsyopAuthorize {
        name: psyop_name.to_string(),
    };
    webview::mode_data_dir(handle, &mode)
        .join("handles")
        .join(twid)
        .join("auth.json")
}

/// Scan every sibling psyop's data dir and return the name of
/// the first one (other than `current`) whose
/// `handles/<twid>/auth.json` exists. Sorting `read_dir` entries
/// alphabetically gives deterministic conflict reporting if
/// multiple owners exist.
///
/// Returns `None` when no conflict, `twid` is empty, or the
/// `psyop/` root doesn't exist yet (first-ever run).
///
/// Cheap: a couple of `read_dir` + per-entry `Path::exists`
/// calls. Fine to call on every cookies kick.
pub fn find_other_psyop_owning_twid(
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
    let psyop_root = webview::mode_data_dir(handle, &current_mode).parent()?.to_path_buf();
    let mut entries: Vec<PathBuf> = fs::read_dir(&psyop_root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    for path in entries {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name == current {
            continue;
        }
        if path.join("handles").join(twid).join("auth.json").exists() {
            return Some(name.to_string());
        }
    }
    None
}

fn write_auth_json(
    handle: &AppHandle<Wry>,
    psyop_name: &str,
    twid: &str,
    tokens: &Tokens,
) -> Result<(), String> {
    let path = auth_json_path(handle, psyop_name, twid);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("serialize tokens: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json + "\n").map_err(|e| format!("write tmp: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}
