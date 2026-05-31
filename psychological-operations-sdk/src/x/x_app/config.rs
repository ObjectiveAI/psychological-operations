//! `x_app.json` — the master X dev-account App's credentials,
//! captured by the chromium extension during `x_app setup` and
//! consumed by the per-psyop OAuth flow.
//!
//! File path: `<config-base-dir>/x_app.json`.
//!
//! `merge` semantics on insert: every `Some(_)` in the incoming
//! payload wins; `None`s preserve the existing value. This lets
//! the operator re-click the extension's "Save credentials" button
//! after a partial paste without clobbering previously-captured
//! fields.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::x::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XAppConfig {
    /// OAuth 2.0 user-context Client ID. Load-bearing — the
    /// per-psyop OAuth flow uses this as `client_id` in the PKCE
    /// authorize redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth 2.0 user-context Client Secret. Used for confidential-
    /// client token exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// App-only Bearer token. Used by `crate::x::client::Client::app_only`
    /// for read-only endpoints (search, tweet lookup) that don't need
    /// user context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    /// RFC 3339 timestamp of the last successful save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

impl XAppConfig {
    /// Returns true iff the load-bearing OAuth 2.0 fields are
    /// present. Per-psyop OAuth (PKCE) needs both `client_id` and
    /// `client_secret` to drive the authorize redirect + token
    /// exchange.
    pub fn is_complete(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }
}

/// Load + assert that `x_app.json` is set up. Returns the loaded
/// config on success, or a clear error pointing the operator at
/// `psychological-operations x_app setup`.
pub fn ensure_setup(config_base_dir: &Path) -> Result<XAppConfig, Error> {
    let cfg = load(config_base_dir)?;
    if !cfg.is_complete() {
        return Err(Error::Other(
            "X App not set up — run `psychological-operations x_app setup` \
             and capture client_id + client_secret before running psyops".into(),
        ));
    }
    Ok(cfg)
}

/// `<config-base-dir>/plugins/psychological-operations/x_app.json`.
/// `config_base_dir` is the outer root (objectiveai's base — same
/// convention as `psychological_operations_sdk::browser::auth_json`'s
/// paths). The `plugins/psychological-operations/` suffix is added
/// here so callers can pass the same path argument they pass to the
/// SDK.
pub fn path(config_base_dir: &Path) -> PathBuf {
    config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("x_app.json")
}

pub fn load(config_base_dir: &Path) -> Result<XAppConfig, Error> {
    let p = path(config_base_dir);
    if !p.exists() {
        return Ok(XAppConfig::default());
    }
    let data = std::fs::read_to_string(&p).map_err(io_err)?;
    serde_json::from_str(&data).map_err(json_err)
}

pub fn save(cfg: &XAppConfig, config_base_dir: &Path) -> Result<(), Error> {
    let p = path(config_base_dir);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(json_err)?;
    std::fs::write(&p, json + "\n").map_err(io_err)
}

/// Returns the merge of `existing` and `incoming` per the
/// "Some-wins, None-preserves" rule. `incoming.saved_at` always
/// wins (caller is expected to stamp it to `now`).
pub fn merge(existing: XAppConfig, incoming: XAppConfig) -> XAppConfig {
    XAppConfig {
        client_id:     incoming.client_id.or(existing.client_id),
        client_secret: incoming.client_secret.or(existing.client_secret),
        bearer_token:  incoming.bearer_token.or(existing.bearer_token),
        saved_at:      incoming.saved_at.or(existing.saved_at),
    }
}

fn io_err(e: std::io::Error) -> Error {
    Error::Other(format!("x_app.json io: {e}"))
}
fn json_err(e: serde_json::Error) -> Error {
    Error::Other(format!("x_app.json json: {e}"))
}
