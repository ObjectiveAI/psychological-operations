//! OAuth 2.0 driver for `Mode::TwitchAuthorize`.
//!
//! Spawns a local callback server, navigates the CEF surface to Twitch's
//! authorize URL (Twitch presents its own login + consent — no signed-in
//! cookie to wait on, unlike X), awaits the redirect, exchanges the code for
//! tokens, validates them for the account identity, and emits the minted
//! bundle as [`Output::TwitchAuthorizeSucceeded`] for the CLI to persist. The
//! browser itself never touches the DB.
//!
//! Simpler than [`crate::authorize`]: the flow is kicked ONCE at startup from
//! `lib.rs` (guarded by a one-shot flag) rather than driven off a cookies
//! snapshot, and the token exchange uses Twitch's confidential form-param flow
//! (client_id + client_secret in the POST body) instead of HTTP Basic auth.
//! The callback-server + PKCE helpers mirror [`crate::authorize`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use psychological_operations_sdk::browser::mode::Mode;
use psychological_operations_sdk::browser::output::{Output, TwitchTokens};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Wry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use urlencoding::encode as urlenc;

use crate::args::Args;
use crate::cef;

const SCOPES: &str = "chat:read chat:edit user:read:chat user:write:chat user:bot";
const AUTHORIZE_BASE: &str = "https://id.twitch.tv/oauth2/authorize";
const TOKEN_ENDPOINT: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_ENDPOINT: &str = "https://id.twitch.tv/oauth2/validate";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
/// Fixed loopback port for the OAuth callback server. MUST stay in sync with
/// the redirect_uri registered in the Twitch app — the TwitchApp overlay's
/// instruction pointer tells the operator to register EXACTLY
/// [`DEFAULT_REDIRECT_URI`]. Twitch requires an exact redirect_uri match, so an
/// ephemeral port could never line up with the pre-registered callback.
const CALLBACK_PORT: u16 = 17563;
/// Default redirect URI (used when `--twitch-redirect-uri` is absent). Must
/// match both the TwitchApp overlay's registration instruction and the port
/// the callback server binds above.
const DEFAULT_REDIRECT_URI: &str = "http://localhost:17563/psychological-operations/callback";

// =================================================================
// One-shot flag — the flow is kicked exactly once per process.
// =================================================================
fn in_flight() -> &'static AtomicBool {
    static SLOT: OnceLock<AtomicBool> = OnceLock::new();
    SLOT.get_or_init(|| AtomicBool::new(false))
}

// =================================================================
// Public entry point — called once at startup from `lib.rs` when
// `Mode::TwitchAuthorize` is the locked mode.
// =================================================================
pub async fn start_flow(handle: &AppHandle<Wry>) {
    match psychological_operations_sdk::browser::mode::get() {
        Some(Mode::TwitchAuthorize { .. }) => {}
        _ => return,
    }
    // Only ever kick a single flow per process.
    if in_flight().swap(true, Ordering::SeqCst) {
        return;
    }

    let handle_for_task = handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_flow(handle_for_task).await {
            let _ = Output::Log {
                message: format!("twitch authorize: flow failed: {e}"),
            }
            .emit();
            let _ = Output::TwitchAuthorizeFailed { error: e }.emit();
            in_flight().store(false, Ordering::SeqCst);
        }
    });
}

async fn run_flow(handle: AppHandle<Wry>) -> Result<(), String> {
    let (client_id, client_secret, redirect_uri) = read_twitch_app_creds(&handle)?;
    let pkce = pkce_generate();
    let state_nonce = random_state();
    let callback_fut = bind_callback_server(CALLBACK_TIMEOUT)
        .await
        .map_err(|e| format!("bind callback server: {e}"))?;
    let authorize_url = build_authorize_url(
        &client_id,
        &redirect_uri,
        &state_nonce,
        &pkce.code_challenge,
    );

    let _ = Output::Log {
        message: format!("twitch authorize: navigating to authorize URL on port {CALLBACK_PORT}"),
    }
    .emit();
    cef::navigate(authorize_url);

    let cb = callback_fut
        .await
        .map_err(|e| format!("await callback: {e}"))?;
    if let Some(err) = cb.error {
        return Err(format!("Twitch returned error on callback: {err}"));
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

    // Validate the freshly-minted token for the account identity (login +
    // user_id). Twitch's token response carries neither, so a follow-up
    // /validate call is the canonical way to learn who we just authorized.
    let validated = validate_token(&tokens.access_token)
        .await
        .map_err(|e| format!("validate token: {e}"))?;

    let _ = Output::Log {
        message: format!(
            "twitch authorize: minted tokens for {} ({}) (expires_at={})",
            validated.login, validated.user_id, tokens.expires_at
        ),
    }
    .emit();
    let _ = Output::TwitchAuthorizeSucceeded {
        user_id: validated.user_id,
        login: validated.login,
        tokens,
    }
    .emit();
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
// PKCE — RFC 7636 §4.2 (verifier + S256 challenge) + state nonce.
// Twitch accepts the confidential (secret) flow with or without PKCE;
// we include it for parity with `crate::authorize`.
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
    Pkce {
        code_verifier,
        code_challenge,
    }
}

fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// =================================================================
// Local callback server — bind 127.0.0.1:CALLBACK_PORT (fixed, to
// match the pre-registered redirect_uri), single accept, hand-parse
// the GET request line. Mirrors `crate::authorize::bind_callback_server`.
// =================================================================
struct Callback {
    code: Option<String>,
    error: Option<String>,
    state: Option<String>,
}

async fn bind_callback_server(
    timeout: Duration,
) -> Result<impl std::future::Future<Output = Result<Callback, String>>, String> {
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .map_err(|e| format!("bind 127.0.0.1:{CALLBACK_PORT}: {e}"))?;

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
            let header =
                std::str::from_utf8(&buf).map_err(|e| format!("non-UTF-8 request: {e}"))?;
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

    Ok(fut)
}

// =================================================================
// Token exchange — POST to Twitch's token endpoint with the client
// creds as FORM PARAMS (Twitch does not use HTTP Basic auth) + the
// PKCE verifier.
// =================================================================
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    /// Twitch returns the granted scopes as an array.
    #[serde(default)]
    scope: Vec<String>,
}

async fn exchange_code_for_tokens(
    client_id: &str,
    client_secret: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TwitchTokens, String> {
    let form = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("build reqwest client: {e}"))?;
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("POST {TOKEN_ENDPOINT}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("token endpoint {status}: {text}"));
    }
    let parsed: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("parse token response: {e}: {text}"))?;
    let expires_at = Utc::now().timestamp() + parsed.expires_in;
    Ok(TwitchTokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        scope: parsed.scope.join(" "),
        expires_at,
    })
}

// =================================================================
// Token validation — GET /oauth2/validate with the literal
// `Authorization: OAuth <token>` header (Twitch uses "OAuth", not
// "Bearer") to learn the authorized account's login + user_id.
// =================================================================
#[derive(Debug, Deserialize)]
struct ValidateResponse {
    login: String,
    user_id: String,
}

async fn validate_token(access_token: &str) -> Result<ValidateResponse, String> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("build reqwest client: {e}"))?;
    let resp = client
        .get(VALIDATE_ENDPOINT)
        .header("Authorization", format!("OAuth {access_token}"))
        .send()
        .await
        .map_err(|e| format!("GET {VALIDATE_ENDPOINT}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("validate endpoint {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("parse validate response: {e}: {text}"))
}

// =================================================================
// Twitch-app OAuth client creds — supplied by the CLI as args (it
// reads them from the captured master-app snapshot). The browser no
// longer touches the DB.
// =================================================================
fn read_twitch_app_creds(handle: &AppHandle<Wry>) -> Result<(String, String, String), String> {
    let args = handle.state::<Args>();
    let client_id = args
        .twitch_client_id
        .clone()
        .ok_or_else(|| "missing --twitch-client-id (required for twitch authorize)".to_string())?;
    let client_secret = args
        .twitch_client_secret
        .clone()
        .ok_or_else(|| "missing --twitch-client-secret".to_string())?;
    let redirect_uri = args
        .twitch_redirect_uri
        .clone()
        .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_string());
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Twitch app client_id or client_secret is empty".into());
    }
    Ok((client_id, client_secret, redirect_uri))
}
