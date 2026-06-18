//! Scraped X dev-account credential HTML (ported from the on-disk
//! `handles/<h>/*.html` snapshots).
//!
//! The two developer-console HTML surfaces (post-create dialog →
//! consumer/secret/bearer; OAuth popup → client id/secret) are captured
//! per normalized handle in `x_app_html`. The HTML *parsers* live in the
//! SDK (`browser::x_app_credentials`); this crate only persists/serves
//! the raw snapshots, which are the single source of truth for the App's
//! credentials.

use crate::{Db, Error};

impl Db {
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

    /// Delete every captured HTML snapshot. Used by
    /// `x-app setup --dangerously-reset` before recapturing.
    pub async fn x_app_html_clear(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM x_app_html")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The active X-App's twid — the single `handle` present in
    /// `x_app_html` (written at `x-app setup`, cleared on x-app reset).
    /// Lets runtime code resolve the active X-App without reading cookies.
    /// `None` when no X-App is set up.
    pub async fn x_app_twid_active(&self) -> Result<Option<String>, Error> {
        let handle: Option<String> =
            sqlx::query_scalar("SELECT handle FROM x_app_html ORDER BY saved_at DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(handle)
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
