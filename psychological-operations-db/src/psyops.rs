//! Psyop definitions (ported from per-psyop git repos + `psyop.json`).
//!
//! Git and commit-versioning were dropped: a psyop is now just
//! `name → definition`, with no history. The `definition` is stored as
//! an opaque JSONB [`serde_json::Value`] (the `PsyOp` struct stays in
//! the SDK/CLI and (de)serializes at the call site). A per-psyop
//! `disabled` flag rides on the same row — it was previously in
//! `config.json`'s per-psyop overrides.

use serde_json::Value;

use crate::{Db, Error};

impl Db {
    /// Insert or replace a psyop's definition. Leaves `disabled`
    /// untouched on update (it's managed independently via
    /// [`Self::psyop_set_disabled`]); a fresh insert defaults it to
    /// `false`. Bumps `updated_at`.
    pub async fn psyop_upsert(&self, name: &str, definition: &Value) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO psyops (name, definition) VALUES ($1, $2) \
             ON CONFLICT (name) DO UPDATE SET \
                 definition = excluded.definition, \
                 updated_at = now()",
        )
        .bind(name)
        .bind(definition)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A psyop's definition, or `None` if no such psyop.
    pub async fn psyop_get(&self, name: &str) -> Result<Option<Value>, Error> {
        let def: Option<Value> =
            sqlx::query_scalar("SELECT definition FROM psyops WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(def)
    }

    /// All psyops as `(name, definition, disabled)`, alphabetical.
    pub async fn psyop_list(&self) -> Result<Vec<(String, Value, bool)>, Error> {
        let rows: Vec<(String, Value, bool)> =
            sqlx::query_as("SELECT name, definition, disabled FROM psyops ORDER BY name ASC")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    /// Whether `name` exists.
    pub async fn psyop_exists(&self, name: &str) -> Result<bool, Error> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM psyops WHERE name = $1)")
                .bind(name)
                .fetch_one(&self.pool)
                .await?;
        Ok(exists)
    }

    /// Set a psyop's `disabled` flag. Returns `true` if the psyop
    /// existed (was updated).
    pub async fn psyop_set_disabled(&self, name: &str, disabled: bool) -> Result<bool, Error> {
        let updated =
            sqlx::query("UPDATE psyops SET disabled = $2, updated_at = now() WHERE name = $1")
                .bind(name)
                .bind(disabled)
                .execute(&self.pool)
                .await?
                .rows_affected();
        Ok(updated > 0)
    }

    /// Whether `name` is disabled. `false` for a missing psyop.
    pub async fn psyop_disabled(&self, name: &str) -> Result<bool, Error> {
        let disabled: Option<bool> =
            sqlx::query_scalar("SELECT disabled FROM psyops WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(disabled.unwrap_or(false))
    }
}
