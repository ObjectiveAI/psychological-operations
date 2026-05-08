//! `billing.json` — the master X dev-account App's credentials,
//! captured by the chrome extension during `billing setup` and
//! consumed by the per-psyop OAuth flow (next commit).
//!
//! File path: `~/.psychological-operations/billing.json`.
//!
//! `merge` semantics on insert: every `Some(_)` in the incoming
//! payload wins; `None`s preserve the existing value. This lets
//! the operator re-click the extension's "Save credentials" button
//! after a partial DOM scrape without clobbering previously-captured
//! fields.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BillingConfig {
    /// OAuth 2.0 user-context Client ID. Load-bearing — the
    /// per-psyop OAuth flow uses this as `client_id` in the PKCE
    /// authorize redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth 2.0 user-context Client Secret. Used for confidential-
    /// client token exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// OAuth 1.0a Consumer Key. Captured opportunistically; not used
    /// by the runtime today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// OAuth 1.0a Consumer Secret. Same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_secret: Option<String>,
    /// App-only Bearer token. Useful as a fallback for read-only
    /// endpoints (search, tweet lookup) that don't need user
    /// context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    /// RFC 3339 timestamp of the last successful save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

impl BillingConfig {
    /// Returns true iff the load-bearing OAuth 2.0 fields are
    /// present. Per-psyop OAuth (PKCE) needs both `client_id` and
    /// `client_secret` to drive the authorize redirect + token
    /// exchange.
    pub fn is_complete(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }
}

/// Load + assert that `billing.json` is set up. Returns the loaded
/// config on success, or a clear error pointing the operator at
/// `psychological-operations billing setup`.
pub fn ensure_setup() -> Result<BillingConfig, Error> {
    let cfg = load()?;
    if !cfg.is_complete() {
        return Err(Error::Other(
            "billing not set up — run `psychological-operations billing setup` \
             and capture client_id + client_secret before running psyops".into(),
        ));
    }
    Ok(cfg)
}

pub fn path() -> PathBuf {
    let home = dirs::home_dir().expect("could not determine home directory");
    home.join(".psychological-operations").join("billing.json")
}

pub fn load() -> Result<BillingConfig, Error> {
    let p = path();
    if !p.exists() {
        return Ok(BillingConfig::default());
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn save(cfg: &BillingConfig) -> Result<(), Error> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&p, json + "\n")?;
    Ok(())
}

/// Returns the merge of `existing` and `incoming` per the
/// "Some-wins, None-preserves" rule. `incoming.saved_at` always
/// wins (caller is expected to stamp it to `now`).
pub fn merge(existing: BillingConfig, incoming: BillingConfig) -> BillingConfig {
    BillingConfig {
        client_id:      incoming.client_id.or(existing.client_id),
        client_secret:  incoming.client_secret.or(existing.client_secret),
        api_key:        incoming.api_key.or(existing.api_key),
        api_key_secret: incoming.api_key_secret.or(existing.api_key_secret),
        bearer_token:   incoming.bearer_token.or(existing.bearer_token),
        saved_at:       incoming.saved_at.or(existing.saved_at),
    }
}
