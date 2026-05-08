//! Per-psyop OAuth 2.0 user-context token store + token-endpoint
//! helpers (authorization-code exchange + refresh).
//!
//! File path: `~/.psychological-operations/tokens/<psyop>.json`.

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Error;

const TOKEN_ENDPOINT: &str = "https://api.x.com/2/oauth2/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token:  String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at:    DateTime<Utc>,
    pub scope:         String,
    pub saved_at:      DateTime<Utc>,
}

pub fn path(psyop: &str) -> PathBuf {
    let home = dirs::home_dir().expect("could not determine home directory");
    home.join(".psychological-operations").join("tokens").join(format!("{psyop}.json"))
}

pub fn load(psyop: &str) -> Result<Option<Tokens>, Error> {
    let p = path(psyop);
    if !p.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(Some(serde_json::from_str(&data)?))
}

pub fn save(psyop: &str, tokens: &Tokens) -> Result<(), Error> {
    let p = path(psyop);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(tokens)?;
    std::fs::write(&p, json + "\n")?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token:  String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in:    i64,
    scope:         String,
    #[allow(dead_code)]
    token_type:    Option<String>,
}

fn basic_auth_header(client_id: &str, client_secret: &str) -> String {
    let raw = format!("{client_id}:{client_secret}");
    format!("Basic {}", BASE64.encode(raw.as_bytes()))
}

fn build_tokens(resp: TokenResponse) -> Tokens {
    let saved_at = Utc::now();
    Tokens {
        access_token:  resp.access_token,
        refresh_token: resp.refresh_token,
        expires_at:    saved_at + Duration::seconds(resp.expires_in),
        scope:         resp.scope,
        saved_at,
    }
}

/// URL-encode a list of key-value pairs as `application/x-www-form-urlencoded`.
fn form_urlencoded(pairs: &[(&str, &str)]) -> String {
    pairs.iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn post_token_endpoint(
    client_id: &str,
    client_secret: &str,
    body: String,
    op_label: &str,
) -> Result<TokenResponse, Error> {
    let client = reqwest::Client::new();
    let resp = client.post(TOKEN_ENDPOINT)
        .header("authorization", basic_auth_header(client_id, client_secret))
        .header("content-type",  "application/x-www-form-urlencoded")
        .body(body)
        .send().await
        .map_err(|e| Error::Other(format!("oauth {op_label} transport error: {e}")))?;
    let status = resp.status();
    let text = resp.text().await
        .map_err(|e| Error::Other(format!("oauth {op_label} body read error: {e}")))?;
    if !status.is_success() {
        return Err(Error::Other(format!(
            "oauth {op_label} failed: {status}: {text}",
        )));
    }
    serde_json::from_str(&text).map_err(|e| Error::Other(format!(
        "oauth {op_label}: malformed response: {e}: {text}",
    )))
}

/// Exchange an authorization code for an access + refresh token.
/// `redirect_uri` must match the one used in the authorize request.
pub async fn exchange_authorization_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, Error> {
    let body = form_urlencoded(&[
        ("grant_type",    "authorization_code"),
        ("code",          code),
        ("redirect_uri",  redirect_uri),
        ("code_verifier", code_verifier),
    ]);
    let parsed = post_token_endpoint(client_id, client_secret, body, "token exchange").await?;
    Ok(build_tokens(parsed))
}

/// Refresh an access token using a refresh token. X rotates the
/// refresh_token on every refresh; the returned `Tokens` carries the
/// new one.
pub async fn refresh(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<Tokens, Error> {
    let body = form_urlencoded(&[
        ("grant_type",    "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id",     client_id),
    ]);
    let parsed = post_token_endpoint(client_id, client_secret, body, "refresh").await?;
    Ok(build_tokens(parsed))
}
