//! Stage-pipeline retry ledger.
//!
//! One row per psyop, holding the stage-pipeline input (`Vec<Post>` as
//! JSONB) of a run whose scoring stages failed. `psyops run` consults this
//! before a normal run: a present entry means "skip everything up to the
//! stages and re-score this saved input". The row is written on stage
//! failure and deleted on stage success. The domain type stays in the
//! CLI; this layer is opaque JSONB (same convention as `psyops`).

use serde_json::Value;

use crate::{unix_now, Db, Error};

impl Db {
    /// Save (or replace) the stage input to retry for `psyop`.
    pub async fn save_stage_retry(&self, psyop: &str, input: &Value) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO stage_retry (psyop, input, at) VALUES ($1, $2, $3) \
             ON CONFLICT (psyop) DO UPDATE SET input = excluded.input, at = excluded.at",
        )
        .bind(psyop)
        .bind(input)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The saved stage input for `psyop`, or `None` if there is no pending
    /// retry.
    pub async fn get_stage_retry(&self, psyop: &str) -> Result<Option<Value>, Error> {
        let out: Option<Value> =
            sqlx::query_scalar("SELECT input FROM stage_retry WHERE psyop = $1")
                .bind(psyop)
                .fetch_optional(&self.pool)
                .await?;
        Ok(out)
    }

    /// Clear the pending retry for `psyop` (no-op if absent).
    pub async fn delete_stage_retry(&self, psyop: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM stage_retry WHERE psyop = $1")
            .bind(psyop)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
