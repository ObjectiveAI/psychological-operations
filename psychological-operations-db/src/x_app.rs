//! Parsed X dev-account credentials (was raw scraped HTML).
//!
//! The two developer-console surfaces (post-create dialog →
//! consumer/secret/bearer; OAuth popup → client id/secret) are parsed at
//! `x-app setup` and their values stored, one row per normalized handle, in
//! `x_app_credentials`. The HTML *parsers* live in the SDK
//! (`browser::x_app_credentials`); this crate only persists/serves the parsed
//! values. Each surface has its own set/get pair; both upsert into the same
//! handle row (the other surface's columns are left untouched).

use sqlx::Row;

use crate::{Db, Error};

impl Db {
    /// Upsert the post-create-dialog credentials for `handle` (consumer key,
    /// secret key, bearer token). Leaves the OAuth-popup columns untouched.
    pub async fn x_app_post_create_set(
        &self,
        handle: &str,
        consumer_key: Option<&str>,
        secret_key: Option<&str>,
        bearer_token: Option<&str>,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO x_app_credentials \
             (handle, consumer_key, secret_key, bearer_token) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (handle) DO UPDATE SET \
                 consumer_key = excluded.consumer_key, \
                 secret_key = excluded.secret_key, \
                 bearer_token = excluded.bearer_token, \
                 saved_at = now()",
        )
        .bind(handle)
        .bind(consumer_key)
        .bind(secret_key)
        .bind(bearer_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert the OAuth-popup credentials for `handle` (client id, client
    /// secret). Leaves the post-create-dialog columns untouched.
    pub async fn x_app_oauth_set(
        &self,
        handle: &str,
        client_id: Option<&str>,
        client_secret: Option<&str>,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO x_app_credentials \
             (handle, client_id, client_secret) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (handle) DO UPDATE SET \
                 client_id = excluded.client_id, \
                 client_secret = excluded.client_secret, \
                 saved_at = now()",
        )
        .bind(handle)
        .bind(client_id)
        .bind(client_secret)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The post-create-dialog credential columns for `handle` as
    /// `(consumer_key, secret_key, bearer_token)`, or `None` if no row exists.
    pub async fn x_app_post_create_get(
        &self,
        handle: &str,
    ) -> Result<Option<(Option<String>, Option<String>, Option<String>)>, Error> {
        let row = sqlx::query(
            "SELECT consumer_key, secret_key, bearer_token \
             FROM x_app_credentials WHERE handle = $1",
        )
        .bind(handle)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| {
            (
                r.get("consumer_key"),
                r.get("secret_key"),
                r.get("bearer_token"),
            )
        }))
    }

    /// The OAuth-popup credential columns for `handle` as
    /// `(client_id, client_secret)`, or `None` if no row exists.
    pub async fn x_app_oauth_get(
        &self,
        handle: &str,
    ) -> Result<Option<(Option<String>, Option<String>)>, Error> {
        let row = sqlx::query(
            "SELECT client_id, client_secret FROM x_app_credentials WHERE handle = $1",
        )
        .bind(handle)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("client_id"), r.get("client_secret"))))
    }

    /// Delete every stored X-App credential row. Used by
    /// `x-app setup --dangerously-reset` before recapturing.
    pub async fn x_app_credentials_clear(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM x_app_credentials")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The active X-App's twid — the most-recently-saved `handle` in
    /// `x_app_credentials` (written at `x-app setup`, cleared on x-app reset).
    /// Lets runtime code resolve the active X-App without reading cookies.
    /// `None` when no X-App is set up.
    pub async fn x_app_twid_active(&self) -> Result<Option<String>, Error> {
        let handle: Option<String> = sqlx::query_scalar(
            "SELECT handle FROM x_app_credentials ORDER BY saved_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(handle)
    }
}
