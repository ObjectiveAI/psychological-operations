//! Two-tier per-key mutex — in-process tokio mutex + cross-process
//! SQLite `locks` table. Lifted out of `cache.rs` so the auth-file
//! coordinator can reuse the same machinery.
//!
//! ```text
//!   TIER 1  (in-process, per key, fast)
//!     DashMap<key, Arc<tokio::sync::Mutex<()>>>
//!     ↓ winner of the local race ↓
//!   TIER 2  (cross-process, per key, slower)
//!     `locks` table — holder_id + TTL + 1 s refresher + GC sweep
//! ```
//!
//! Cross-process algorithm — UNCHANGED from the prior cache lock:
//! per-key exclusive mutex via a `locks` table row with a 16-byte
//! random `holder_id`, a 10 s `expires_at` TTL, a 1 s refresher
//! task re-stamping `expires_at` while alive, and a GC sweep that
//! deletes expired rows on the next acquire.
//!
//! Release ordering — load-bearing. Both [`LockGuard::release`]
//! and [`LockGuard`]'s `Drop` perform a real `DELETE FROM locks`
//! (not a TTL refresh) BEFORE the in-process mutex guard drops, so
//! a freshly-woken in-process contender sees a clean SQLite slot
//! and does NOT spin-poll the locks table.
//!
//! Multiple `Locker` instances can share one `SqlitePool` (i.e.
//! one SQLite file). They use independent in-process key spaces
//! (separate `DashMap`s), but contend on the same cross-process
//! `locks` table — so callers must namespace their keys (e.g.
//! `SHA-256("cache:" ‖ …)` vs `SHA-256("auth:" ‖ …)`) to avoid
//! collisions between concerns sharing the file.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rand::RngCore;
use sqlx::sqlite::SqlitePool;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use super::Error;

/// Cross-process per-key mutex TTL.
pub(crate) const LOCK_TTL_SECS: i64 = 10;
pub(crate) const LOCK_REFRESH_SECS: u64 = 1;
pub(crate) const LOCK_POLL_INTERVAL_MS: u64 = 50;

/// Two-tier locker. Cheap to clone (Arc'd internals). One instance
/// per concern (cache, auth) — they may share the same
/// `SqlitePool` but get distinct in-process key spaces via their
/// own `DashMap`s.
#[derive(Debug, Clone)]
pub(crate) struct Locker {
    pool: SqlitePool,
    inproc_locks: Arc<DashMap<[u8; 32], Arc<Mutex<()>>>>,
}

/// Held lock on one key — covers both tiers. Prefer
/// [`LockGuard::release`] over `drop(guard)` when you can
/// `.await`: release waits for the SQLite row deletion to land
/// before yielding, which gives the next in-process contender a
/// clean cross-process slot with zero polling.
pub struct LockGuard {
    pool: SqlitePool,
    key: [u8; 32],
    holder_id: [u8; 16],
    refresher: Option<JoinHandle<()>>,
    /// Held while the guard lives. Field order matters: this is
    /// declared LAST so it drops LAST in `release`, after the
    /// SQLite DELETE has been awaited.
    inproc_guard: Option<OwnedMutexGuard<()>>,
}

impl LockGuard {
    /// Release in the canonical order: stop the refresher, delete
    /// the SQLite locks row (awaited), then drop the in-process
    /// mutex guard.
    pub async fn release(mut self) {
        if let Some(h) = self.refresher.take() {
            h.abort();
        }
        let _ = sqlx::query("DELETE FROM locks WHERE key = ? AND holder_id = ?")
            .bind(&self.key[..])
            .bind(&self.holder_id[..])
            .execute(&self.pool)
            .await;
        // inproc_guard drops at end of fn — after the await above.
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Fallback when caller forgot `release().await`. Transfer
        // the inproc guard into a detached tokio task that drops
        // it AFTER the SQLite DELETE completes. Preserves the
        // "SQLite row gone before inproc released" invariant.
        if let Some(h) = self.refresher.take() {
            h.abort();
        }
        let Some(inproc) = self.inproc_guard.take() else {
            return;
        };
        let pool = self.pool.clone();
        let key = self.key;
        let holder_id = self.holder_id;
        // Panics if no tokio runtime is alive at Drop time. The
        // cache/auth callers are all tokio-rooted so this holds.
        tokio::spawn(async move {
            let _ = sqlx::query("DELETE FROM locks WHERE key = ? AND holder_id = ?")
                .bind(&key[..])
                .bind(&holder_id[..])
                .execute(&pool)
                .await;
            drop(inproc);
        });
    }
}

impl Locker {
    /// Build a Locker around an existing pool. Caller is
    /// responsible for having called [`Locker::ensure_schema`] at
    /// some earlier point (`Locker::new` is cheap and idempotent —
    /// schema creation is one-shot per file).
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            inproc_locks: Arc::new(DashMap::new()),
        }
    }

    /// Idempotent — creates the `locks` table if it doesn't exist.
    /// Multiple Locker instances over the same pool will see the
    /// same table.
    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS locks (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 holder_id BLOB NOT NULL,\
                 expires_at INTEGER NOT NULL\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("locker schema: {e}")))?;
        Ok(())
    }

    /// Acquire the two-tier per-key lock.
    pub(crate) async fn acquire(&self, key: &[u8; 32]) -> Result<LockGuard, Error> {
        // --- TIER 1: in-process per-key tokio Mutex ---
        let inproc_arc = self
            .inproc_locks
            .entry(*key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let inproc_guard = inproc_arc.lock_owned().await;

        // --- TIER 2: cross-process SQLite locks-table ---
        let mut holder_id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut holder_id);

        loop {
            let acquired = {
                let mut tx = self
                    .pool
                    .begin()
                    .await
                    .map_err(|e| Error::Other(format!("locker tx: {e}")))?;
                let now = unix_now();
                sqlx::query("DELETE FROM locks WHERE key = ? AND expires_at <= ?")
                    .bind(&key[..])
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| Error::Other(format!("locker gc: {e}")))?;
                let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM locks WHERE key = ?")
                    .bind(&key[..])
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| Error::Other(format!("locker probe: {e}")))?;
                if live > 0 {
                    drop(tx);
                    false
                } else {
                    sqlx::query(
                        "INSERT INTO locks (key, holder_id, expires_at) VALUES (?, ?, ?)",
                    )
                    .bind(&key[..])
                    .bind(&holder_id[..])
                    .bind(now + LOCK_TTL_SECS)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| Error::Other(format!("locker insert: {e}")))?;
                    tx.commit()
                        .await
                        .map_err(|e| Error::Other(format!("locker commit: {e}")))?;
                    true
                }
            };
            if acquired {
                let refresher = spawn_refresher(self.pool.clone(), *key, holder_id);
                return Ok(LockGuard {
                    pool: self.pool.clone(),
                    key: *key,
                    holder_id,
                    refresher: Some(refresher),
                    inproc_guard: Some(inproc_guard),
                });
            }
            tokio::time::sleep(Duration::from_millis(LOCK_POLL_INTERVAL_MS)).await;
        }
    }
}

fn spawn_refresher(
    pool: SqlitePool,
    key: [u8; 32],
    holder_id: [u8; 16],
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(LOCK_REFRESH_SECS)).await;
            let now = unix_now();
            let _ = sqlx::query(
                "UPDATE locks SET expires_at = ? \
                 WHERE key = ? AND holder_id = ?",
            )
            .bind(now + LOCK_TTL_SECS)
            .bind(&key[..])
            .bind(&holder_id[..])
            .execute(&pool)
            .await;
        }
    })
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
