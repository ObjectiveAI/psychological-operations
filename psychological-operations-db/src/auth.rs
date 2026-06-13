//! Per-persona OAuth token storage (ported from the on-disk
//! `auth.json` files).
//!
//! Tokens are keyed by `(kind, name, persona_twid, x_app_twid)`:
//!   - `kind` — `"psyop"` or `"agent"` (the SDK's `PersonaKind`),
//!   - `name` — the named persona,
//!   - `persona_twid` — the signed-in X user the tokens authorize,
//!   - `x_app_twid` — the master X-App that minted the OAuth creds.
//!
//! Each `(persona, X-App)` pair gets its own independent token store, so
//! swapping the signed-in X-App routes to a different row. The token
//! bundle (`Tokens`: access/refresh/expiry/scope) is stored as opaque
//! JSONB — the struct + freshness logic stay in the SDK.
//!
//! Read-modify-write callers should serialize via [`Db::lock`] (the
//! advisory locker) on a key derived from this tuple, the same way the
//! response cache guards its entries.

use serde_json::Value;

use crate::{Db, Error};

impl Db {
    /// Load the token bundle for a persona × X-App pair, or `None` if
    /// none has been minted yet.
    pub async fn auth_get(
        &self,
        kind: &str,
        name: &str,
        persona_twid: &str,
        x_app_twid: &str,
    ) -> Result<Option<Value>, Error> {
        let tokens: Option<Value> = sqlx::query_scalar(
            "SELECT tokens FROM auth_tokens \
             WHERE kind = $1 AND name = $2 AND persona_twid = $3 AND x_app_twid = $4",
        )
        .bind(kind)
        .bind(name)
        .bind(persona_twid)
        .bind(x_app_twid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(tokens)
    }

    /// Insert or replace the token bundle for a persona × X-App pair.
    pub async fn auth_set(
        &self,
        kind: &str,
        name: &str,
        persona_twid: &str,
        x_app_twid: &str,
        tokens: &Value,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO auth_tokens \
                 (kind, name, persona_twid, x_app_twid, tokens) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (kind, name, persona_twid, x_app_twid) DO UPDATE SET \
                 tokens = excluded.tokens, updated_at = now()",
        )
        .bind(kind)
        .bind(name)
        .bind(persona_twid)
        .bind(x_app_twid)
        .bind(tokens)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete every token row for one persona (all persona_twid ×
    /// x_app_twid leaves). Used by the `--dangerously-reset` login path.
    pub async fn auth_delete_persona(&self, kind: &str, name: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM auth_tokens WHERE kind = $1 AND name = $2")
            .bind(kind)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete every token row across all personas. Used by
    /// `x_app setup --dangerously-reset` (a new X-App orphans every
    /// persona's tokens).
    pub async fn auth_delete_all(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM auth_tokens")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
