//! The master X dev-account App's credentials — captured by the
//! chromium extension during `x_app setup` and consumed by the
//! per-psyop OAuth flow.
//!
//! Read path for the db crate's `x_app` singleton row (was `x_app.json`):
//! the [`XAppConfig`] shape + [`load`] + the completeness check. Storage
//! is the db's [`XAppRow`].

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
}

pub async fn load(db: &Db) -> Result<XAppConfig, Error> {
    let row = db.x_app_get().await?;
    Ok(XAppConfig::from_row(row))
}
