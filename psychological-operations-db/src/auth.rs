//! Persona identity + OAuth token storage, split across two tables.
//!
//! - **`persona_twids`** maps a persona `(kind, name)` → the X account
//!   `persona_twid` it operates. Established by the login browser once it
//!   observes the signed-in `twid` cookie; read by every runtime auth
//!   decision so nothing outside the browser has to touch cookies.
//!   - `kind` — `"psyop"` or `"agent"` (the SDK's `PersonaKind`),
//!   - `name` — the named persona,
//!   - `persona_twid` — the signed-in X user the persona operates as.
//!
//! - **`account_auth`** maps an account `persona_twid` → its OAuth token
//!   bundle. Keyed by `persona_twid` alone: an X-App reset wipes the whole
//!   table, so only one X-App's tokens ever exist at a time, which makes
//!   `persona_twid` unique. `x_app_twid` rides along as a non-key column
//!   for token refresh + provenance. The bundle (`Tokens`:
//!   access/refresh/expiry/scope) is opaque JSONB — the struct + freshness
//!   logic stay in the SDK.
//!
//! Read-modify-write token callers should serialize via [`Db::lock`] (the
//! advisory locker) on a key derived from the `persona_twid`, the same way
//! the response cache guards its entries.

use serde_json::Value;

use crate::{Db, Error};

impl Db {
    // ---- persona_twids: persona (kind, name) -> account twid ----------

    /// The account twid a persona operates as, or `None` if the persona
    /// has never been signed in (no mapping established).
    pub async fn persona_twid_get(&self, kind: &str, name: &str) -> Result<Option<String>, Error> {
        let twid: Option<String> = sqlx::query_scalar(
            "SELECT persona_twid FROM persona_twids WHERE kind = $1 AND name = $2",
        )
        .bind(kind)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(twid)
    }

    /// Establish (or update) the persona → account-twid mapping. Called by
    /// the login browser once it observes the signed-in `twid`.
    pub async fn persona_twid_set(
        &self,
        kind: &str,
        name: &str,
        persona_twid: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO persona_twids (kind, name, persona_twid) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (kind, name) DO UPDATE SET \
                 persona_twid = excluded.persona_twid, updated_at = now()",
        )
        .bind(kind)
        .bind(name)
        .bind(persona_twid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete one persona's account mapping. Used by the agent/psyop
    /// `--dangerously-reset` login path. Leaves `account_auth` untouched —
    /// the account's token may still be used by another persona.
    pub async fn persona_twid_delete(&self, kind: &str, name: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM persona_twids WHERE kind = $1 AND name = $2")
            .bind(kind)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// First other persona (alphabetical, `name <> exclude_name`) of the
    /// same `kind` mapped to `persona_twid`. Powers the browser's
    /// cross-psyop twid-ownership guard. `None` when no other owner exists.
    pub async fn persona_twid_find_other_owner(
        &self,
        kind: &str,
        persona_twid: &str,
        exclude_name: &str,
    ) -> Result<Option<String>, Error> {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM persona_twids \
             WHERE kind = $1 AND persona_twid = $2 AND name <> $3 \
             ORDER BY name ASC LIMIT 1",
        )
        .bind(kind)
        .bind(persona_twid)
        .bind(exclude_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(name)
    }

    // ---- account_auth: account twid -> OAuth token -------------------

    /// Load the token bundle for an account twid, or `None` if none has
    /// been minted yet.
    pub async fn account_auth_get(&self, persona_twid: &str) -> Result<Option<Value>, Error> {
        let tokens: Option<Value> =
            sqlx::query_scalar("SELECT tokens FROM account_auth WHERE persona_twid = $1")
                .bind(persona_twid)
                .fetch_optional(&self.pool)
                .await?;
        Ok(tokens)
    }

    /// Insert or replace the token bundle for an account twid. `x_app_twid`
    /// is stored for refresh + provenance but is not part of the key.
    pub async fn account_auth_set(
        &self,
        persona_twid: &str,
        x_app_twid: &str,
        tokens: &Value,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO account_auth (persona_twid, x_app_twid, tokens) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (persona_twid) DO UPDATE SET \
                 x_app_twid = excluded.x_app_twid, \
                 tokens = excluded.tokens, updated_at = now()",
        )
        .bind(persona_twid)
        .bind(x_app_twid)
        .bind(tokens)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete every account token row. Used by `x-app setup
    /// --dangerously-reset` (a new X-App orphans every account's tokens).
    pub async fn account_auth_delete_all(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM account_auth")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
