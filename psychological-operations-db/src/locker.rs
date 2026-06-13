//! Two-tier per-key mutex: an in-process tokio mutex (tier 1, fast,
//! avoids a DB round-trip for same-process contenders) over a postgres
//! **advisory lock** (tier 2, cross-process). Replaces the old SQLite
//! `locks` table + TTL + refresher + GC + poll loop — postgres releases
//! the advisory lock automatically if the holding session dies, so none
//! of that lease machinery is needed.
//!
//! The lock is session-scoped (`pg_advisory_lock`), held on a dedicated
//! pooled connection for the guard's lifetime, and **explicitly**
//! unlocked before that connection returns to the pool — a pooled
//! session is reused, not closed, so it would otherwise stay locked.

use std::sync::Arc;

use sqlx::Postgres;
use sqlx::pool::PoolConnection;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::{Db, Error};

/// Fold a 32-byte cache/auth key into the `bigint` keyspace postgres
/// advisory locks use. A collision only causes occasional *false*
/// serialization between two unrelated keys — correctness is preserved.
fn key_to_i64(key: &[u8; 32]) -> i64 {
    i64::from_le_bytes(key[..8].try_into().expect("32 >= 8"))
}

/// Held two-tier lock. Prefer [`LockGuard::release`] (awaits the
/// advisory unlock) over letting it drop.
pub struct LockGuard {
    key: i64,
    /// The connection holding the advisory lock; `Some` until released.
    conn: Option<PoolConnection<Postgres>>,
    /// Tier-1 guard; declared after `conn` so it drops last.
    inproc_guard: Option<OwnedMutexGuard<()>>,
}

impl LockGuard {
    /// Unlock (awaited) then release the connection + in-process guard.
    pub async fn release(mut self) {
        if let Some(mut conn) = self.conn.take() {
            let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(self.key)
                .execute(&mut *conn)
                .await;
            // conn drops here → returns to the pool, now unlocked.
        }
        // inproc_guard drops at end of scope, after the unlock await.
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Fallback when the caller didn't `release().await`. Move the
        // connection into a task that unlocks before returning it to
        // the pool (else the pooled session stays locked). Requires a
        // live tokio runtime — every caller is tokio-rooted.
        let Some(mut conn) = self.conn.take() else { return };
        let key = self.key;
        let inproc = self.inproc_guard.take();
        tokio::spawn(async move {
            let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(key)
                .execute(&mut *conn)
                .await;
            drop(conn);
            drop(inproc);
        });
    }
}

impl Db {
    /// Acquire the two-tier per-key lock. Blocks (postgres-side) until
    /// the advisory lock is free.
    pub async fn lock(&self, key: &[u8; 32]) -> Result<LockGuard, Error> {
        let ikey = key_to_i64(key);

        // Tier 1: in-process per-key mutex.
        let inproc_arc = self
            .inproc_locks
            .entry(ikey)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let inproc_guard = inproc_arc.lock_owned().await;

        // Tier 2: postgres advisory lock on a held connection.
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(ikey)
            .execute(&mut *conn)
            .await?;

        Ok(LockGuard {
            key: ikey,
            conn: Some(conn),
            inproc_guard: Some(inproc_guard),
        })
    }
}
