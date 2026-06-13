//! The master X dev-account App's credentials — captured by the
//! chromium extension during `x_app setup` and consumed by the
//! per-psyop OAuth flow.
//!
//! Persisted in the db crate's `x_app` singleton row (was `x_app.json`).
//! The [`XAppConfig`] shape + the merge semantics stay here; storage is
//! the db's [`XAppRow`].
//!
//! `merge` semantics on insert: every `Some(_)` in the incoming payload
//! wins; `None`s preserve the existing value. This lets the operator
//! re-click the extension's "Save credentials" button after a partial
//! paste without clobbering previously-captured fields.

use psychological_operations_db::{Db, XAppRow};
use serde::{Deserialize, Serialize};

use crate::x::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XAppConfig {
    /// OAuth 2.0 user-context Client ID. Load-bearing — the per-psyop
    /// OAuth flow uses this as `client_id` in the PKCE authorize
    /// redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth 2.0 user-context Client Secret. Used for confidential-
    /// client token exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// App-only Bearer token. Used by `Client::new(..., AuthMode::XApp)`
    /// for read-only endpoints (search, tweet lookup) that don't need
    /// user context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    /// RFC 3339 timestamp of the last successful save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

impl XAppConfig {
    /// Returns true iff the load-bearing OAuth 2.0 fields are present.
    /// Per-psyop OAuth (PKCE) needs both `client_id` and `client_secret`
    /// to drive the authorize redirect + token exchange.
    pub fn is_complete(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }

    fn from_row(row: XAppRow) -> Self {
        Self {
            client_id: row.client_id,
            client_secret: row.client_secret,
            bearer_token: row.bearer_token,
            saved_at: row.saved_at,
        }
    }

    fn to_row(&self) -> XAppRow {
        XAppRow {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            bearer_token: self.bearer_token.clone(),
            saved_at: self.saved_at.clone(),
        }
    }
}

/// Load + assert that the X-App is set up. Returns the loaded config on
/// success, or a clear error pointing the operator at
/// `psychological-operations x_app setup`.
pub async fn ensure_setup(db: &Db) -> Result<XAppConfig, Error> {
    let cfg = load(db).await?;
    if !cfg.is_complete() {
        return Err(Error::Other(
            "X App not set up — run `psychological-operations x_app setup` \
             and capture client_id + client_secret before running psyops"
                .into(),
        ));
    }
    Ok(cfg)
}

pub async fn load(db: &Db) -> Result<XAppConfig, Error> {
    let row = db.x_app_get().await?;
    Ok(XAppConfig::from_row(row))
}

pub async fn save(db: &Db, cfg: &XAppConfig) -> Result<(), Error> {
    db.x_app_set(&cfg.to_row()).await?;
    Ok(())
}

/// Returns the merge of `existing` and `incoming` per the "Some-wins,
/// None-preserves" rule. `incoming.saved_at` always wins (caller is
/// expected to stamp it to `now`).
pub fn merge(existing: XAppConfig, incoming: XAppConfig) -> XAppConfig {
    XAppConfig {
        client_id: incoming.client_id.or(existing.client_id),
        client_secret: incoming.client_secret.or(existing.client_secret),
        bearer_token: incoming.bearer_token.or(existing.bearer_token),
        saved_at: incoming.saved_at.or(existing.saved_at),
    }
}
