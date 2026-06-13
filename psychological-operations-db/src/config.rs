//! Delivery-target configuration (ported from `config.json`).
//!
//! Two ordered lists of targets — a global list that fires on every
//! psyop run, and a per-psyop list — live in `global_targets` /
//! `psyop_targets`. `ord` is a gapless 0-based index matching the
//! `targets list/add/del <index>` CLI surface. Each target is stored as
//! an opaque JSONB [`serde_json::Value`]; the `Destination` type stays
//! in the CLI and (de)serializes at the call site (storage-only crate).
//!
//! The per-psyop *disabled* flag lives on the `psyops` table, not here —
//! see [`crate::psyops`].

use serde_json::Value;

use crate::{Db, Error};

impl Db {
    /// Global targets in `ord` order. Empty after an operator deletes
    /// them all (distinct from "never seeded" — see
    /// [`Self::seed_global_targets_if_unseeded`]).
    pub async fn global_targets(&self) -> Result<Vec<Value>, Error> {
        let rows: Vec<Value> =
            sqlx::query_scalar("SELECT target FROM global_targets ORDER BY ord ASC")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    /// Replace the entire global target list with `targets`, reindexed
    /// `0..targets.len()`. Atomic (single transaction).
    pub async fn set_global_targets(&self, targets: &[Value]) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM global_targets")
            .execute(&mut *tx)
            .await?;
        for (ord, target) in targets.iter().enumerate() {
            sqlx::query("INSERT INTO global_targets (ord, target) VALUES ($1, $2)")
                .bind(ord as i32)
                .bind(target)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Per-psyop targets in `ord` order.
    pub async fn psyop_targets(&self, psyop: &str) -> Result<Vec<Value>, Error> {
        let rows: Vec<Value> = sqlx::query_scalar(
            "SELECT target FROM psyop_targets WHERE psyop = $1 ORDER BY ord ASC",
        )
        .bind(psyop)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Replace one psyop's target list, reindexed `0..targets.len()`.
    /// Atomic.
    pub async fn set_psyop_targets(&self, psyop: &str, targets: &[Value]) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM psyop_targets WHERE psyop = $1")
            .bind(psyop)
            .execute(&mut *tx)
            .await?;
        for (ord, target) in targets.iter().enumerate() {
            sqlx::query("INSERT INTO psyop_targets (psyop, ord, target) VALUES ($1, $2, $3)")
                .bind(psyop)
                .bind(ord as i32)
                .bind(target)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// First-run default: if the global target list has never been
    /// seeded, insert `defaults` and mark it seeded — all in one
    /// transaction so concurrent first runs can't double-seed. No-op
    /// once seeded, even if the operator later empties the list. The
    /// CLI passes its `[X::Like, Stdout]` default here.
    pub async fn seed_global_targets_if_unseeded(&self, defaults: &[Value]) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        let seeded: bool = sqlx::query_scalar(
            "SELECT COALESCE(\
                 (SELECT global_targets_seeded FROM config_state WHERE singleton), \
                 false)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !seeded {
            for (ord, target) in defaults.iter().enumerate() {
                sqlx::query("INSERT INTO global_targets (ord, target) VALUES ($1, $2)")
                    .bind(ord as i32)
                    .bind(target)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query(
                "INSERT INTO config_state (singleton, global_targets_seeded) \
                 VALUES (true, true) \
                 ON CONFLICT (singleton) DO UPDATE SET global_targets_seeded = true",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
