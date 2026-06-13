//! Master X dev-account App credentials + scraped credential HTML
//! (ported from `x_app.json` and the on-disk `handles/<h>/*.html`
//! snapshots).
//!
//! Credentials live in the `x_app` singleton row (one App per
//! deployment). The merge-on-save semantics ("Some-wins,
//! None-preserves") stay in the SDK's `XAppConfig`; this crate just
//! stores/loads the four nullable fields via the [`XAppRow`] DTO.
//!
//! The two developer-console HTML surfaces (post-create dialog, OAuth
//! popup) are captured per normalized handle in `x_app_html`. The HTML
//! *parsers* stay in the SDK (`browser::x_app_credentials`); this crate
//! only persists/serves the raw snapshots.

use crate::{Db, Error};

/// The four nullable columns of the `x_app` singleton. Maps to/from the
/// SDK's `XAppConfig` at the call site.
#[derive(Debug, Clone, Default)]
pub struct XAppRow {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub bearer_token: Option<String>,
    pub saved_at: Option<String>,
}

impl Db {
    /// Load the X-App credential singleton. `XAppRow::default()` (all
    /// `None`) if never saved.
    pub async fn x_app_get(&self) -> Result<XAppRow, Error> {
        let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT client_id, client_secret, bearer_token, saved_at \
                 FROM x_app WHERE singleton",
            )
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some((client_id, client_secret, bearer_token, saved_at)) => XAppRow {
                client_id,
                client_secret,
                bearer_token,
                saved_at,
            },
            None => XAppRow::default(),
        })
    }

    /// Replace the X-App credential singleton with `row` wholesale. The
    /// caller is responsible for any merge (load → merge → set).
    pub async fn x_app_set(&self, row: &XAppRow) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO x_app (singleton, client_id, client_secret, bearer_token, saved_at) \
             VALUES (true, $1, $2, $3, $4) \
             ON CONFLICT (singleton) DO UPDATE SET \
                 client_id = excluded.client_id, \
                 client_secret = excluded.client_secret, \
                 bearer_token = excluded.bearer_token, \
                 saved_at = excluded.saved_at",
        )
        .bind(row.client_id.as_deref())
        .bind(row.client_secret.as_deref())
        .bind(row.bearer_token.as_deref())
        .bind(row.saved_at.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store (upsert) a developer-console HTML snapshot for `handle`.
    /// `kind` is `"post_create_dialog"` or `"oauth_popup"`.
    pub async fn x_app_html_set(&self, handle: &str, kind: &str, html: &str) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO x_app_html (handle, kind, html, saved_at) \
             VALUES ($1, $2, $3, now()) \
             ON CONFLICT (handle, kind) DO UPDATE SET \
                 html = excluded.html, saved_at = excluded.saved_at",
        )
        .bind(handle)
        .bind(kind)
        .bind(html)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch a stored HTML snapshot, or `None` if none captured yet.
    pub async fn x_app_html_get(&self, handle: &str, kind: &str) -> Result<Option<String>, Error> {
        let html: Option<String> =
            sqlx::query_scalar("SELECT html FROM x_app_html WHERE handle = $1 AND kind = $2")
                .bind(handle)
                .bind(kind)
                .fetch_optional(&self.pool)
                .await?;
        Ok(html)
    }

    /// Presence check for a snapshot — the `state` derivation's
    /// green-dot signal.
    pub async fn x_app_html_present(&self, handle: &str, kind: &str) -> Result<bool, Error> {
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM x_app_html WHERE handle = $1 AND kind = $2)",
        )
        .bind(handle)
        .bind(kind)
        .fetch_one(&self.pool)
        .await?;
        Ok(present)
    }
}
