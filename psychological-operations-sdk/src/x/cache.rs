//! Two-tier response cache for the X v2 API client.
//!
//! Storage is SQLite via `sqlx::SqlitePool`. Bodies live in `cache`
//! keyed by `SHA-256(method ‖ path ‖ query ‖ body)`. Eviction is LRU
//! by `inserted_at` capped at `max_size` bytes (0 disables).
//!
//! Locking is two-tier — load-bearing for the "hundreds of agents
//! fetching the same tweet" case:
//!
//! ```text
//!   TIER 1  (in-process, per key, fast)
//!     DashMap<key, Arc<tokio::sync::Mutex<()>>>
//!     ↓ winner of the local race ↓
//!   TIER 2  (cross-process, per key, slower)
//!     `locks` table — holder_id + TTL + 1 s refresher + GC sweep
//! ```
//!
//! In-process contenders queue cheaply on the local tokio mutex. Only
//! the winner of the in-process race walks the SQLite acquire dance,
//! so the `locks` table effectively coordinates between PROCESSES
//! only. Within a process, an unbounded number of tasks can call
//! [`Cache::get_or_fetch`] for the same key — exactly one fires the
//! fetch closure, the rest read the cached body.
//!
//! ## Cross-process locking algorithm (unchanged from prior version)
//!
//! Per-key exclusive mutex — at most one cross-process holder per
//! key. Different keys lock independently. Acquire is a
//! transactional `DELETE expired → COUNT live → INSERT` cycle; the
//! holder runs a 1 s refresher task that re-stamps `expires_at`
//! while alive. A SIGKILL'd holder's row ages out within
//! `LOCK_TTL_SECS` and the next acquire's GC sweeps it.
//!
//! ## Release ordering
//!
//! Both [`LockGuard::release`] and the [`Drop`] fallback preserve
//! the invariant "SQLite row gone before in-process mutex released":
//! a freshly-woken in-process contender sees a clean SQLite slot
//! and does NOT spin-poll the locks table. Without this, the
//! two-tier design would still work but pay needless polling
//! latency on every in-process handoff.
//!
//! ## Runtime requirement
//!
//! The Drop path spawns a detached tokio task to delete the SQLite
//! row asynchronously while holding the in-process guard until the
//! delete completes. This requires a live tokio runtime at Drop
//! time. The cache is only used from MCP / CLI / browser binaries,
//! all tokio-rooted, so this holds in practice. Callers are still
//! encouraged to prefer `guard.release().await` explicitly when
//! convenient.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rand::RngCore;
use reqwest::Method;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use super::Error;

/// Cross-process per-key mutex TTL. Refreshed every
/// `LOCK_REFRESH_SECS` by the holder's spawned task; aged out by
/// the next acquire's GC if the holder dies.
const LOCK_TTL_SECS: i64 = 10;
const LOCK_REFRESH_SECS: u64 = 1;
const LOCK_POLL_INTERVAL_MS: u64 = 50;

/// SQLite-backed response cache. One file per
/// `<config-base-dir>/plugins/psychological-operations/x-api-cache.sqlite`.
pub struct Cache {
    pool: SqlitePool,
    /// Tier-1 locks. One `Mutex<()>` per key, lazily inserted.
    /// Entries are not evicted — see module-level docs.
    inproc_locks: Arc<DashMap<[u8; 32], Arc<Mutex<()>>>>,
    /// Bytes. When > 0, [`store`] evicts oldest entries (by
    /// `inserted_at`) until total body size ≤ this value. 0
    /// disables eviction.
    max_size: u64,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("max_size", &self.max_size)
            .finish_non_exhaustive()
    }
}

/// Held lock on one key — covers both the in-process and
/// cross-process tiers.
///
/// Prefer [`LockGuard::release`] over `drop(guard)` when you can
/// `.await` — `release` waits for the SQLite row deletion to land
/// before yielding, which gives the next in-process contender a
/// clean cross-process slot with zero polling.
pub struct LockGuard {
    pool: SqlitePool,
    key: [u8; 32],
    holder_id: [u8; 16],
    refresher: Option<JoinHandle<()>>,
    /// Held while the guard lives. Field order matters: this is
    /// declared LAST so it drops LAST in `release`, after the
    /// SQLite DELETE has been awaited. See module-level docs.
    inproc_guard: Option<OwnedMutexGuard<()>>,
}

impl LockGuard {
    /// Release in the canonical order: stop the refresher, delete
    /// the SQLite locks row (awaited), then drop the in-process
    /// mutex guard. The await is the load-bearing piece — without
    /// it, a queued in-process contender would wake up and find
    /// our stale SQLite row, forcing a poll cycle.
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
        // Fallback path when caller forgot release(). We can't
        // await here, so we transfer the inproc guard into a
        // detached tokio task that drops it AFTER the SQLite
        // DELETE completes. Same release ordering as the explicit
        // path above.
        if let Some(h) = self.refresher.take() {
            h.abort();
        }
        let Some(inproc) = self.inproc_guard.take() else {
            return;
        };
        let pool = self.pool.clone();
        let key = self.key;
        let holder_id = self.holder_id;
        // Will panic if no tokio runtime is alive — see module docs.
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

impl Cache {
    /// Open (creating if missing) the cache file under
    /// `<config_base_dir>/plugins/psychological-operations/x-api-cache.sqlite`.
    /// Enables WAL + a 5 s busy timeout so concurrent processes
    /// don't fail with `SQLITE_BUSY` on contention.
    pub async fn open(config_base_dir: &Path, max_size: u64) -> Result<Self, Error> {
        let dir = config_base_dir
            .join("plugins")
            .join("psychological-operations");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Other(format!("cache mkdir: {e}")))?;
        let path = dir.join("x-api-cache.sqlite");

        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| Error::Other(format!("cache open {}: {e}", path.display())))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cache (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 body BLOB NOT NULL,\
                 inserted_at INTEGER NOT NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|e| Error::Other(format!("cache schema: {e}")))?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS locks (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 holder_id BLOB NOT NULL,\
                 expires_at INTEGER NOT NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|e| Error::Other(format!("cache schema: {e}")))?;

        Ok(Self {
            pool,
            inproc_locks: Arc::new(DashMap::new()),
            max_size,
        })
    }

    /// Acquire the two-tier per-key lock. Returns a guard that
    /// releases both tiers on drop (best-effort) or on
    /// [`LockGuard::release`] (canonical, awaited).
    pub async fn lock(&self, key: &[u8; 32]) -> Result<LockGuard, Error> {
        // --- TIER 1: in-process per-key tokio Mutex ---
        // Clone the Arc out of the DashMap entry so we don't hold
        // the DashMap entry guard across the await.
        let inproc_arc = self
            .inproc_locks
            .entry(*key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let inproc_guard = inproc_arc.lock_owned().await;

        // --- TIER 2: cross-process SQLite locks-table ---
        // Algorithm unchanged from the prior rusqlite version. Only
        // the driver call shape changes.
        let mut holder_id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut holder_id);

        loop {
            let acquired = {
                let mut tx = self
                    .pool
                    .begin()
                    .await
                    .map_err(|e| Error::Other(format!("cache lock tx: {e}")))?;
                let now = unix_now();
                sqlx::query("DELETE FROM locks WHERE key = ? AND expires_at <= ?")
                    .bind(&key[..])
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| Error::Other(format!("cache lock gc: {e}")))?;
                let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM locks WHERE key = ?")
                    .bind(&key[..])
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| Error::Other(format!("cache lock probe: {e}")))?;
                if live > 0 {
                    // Someone else (another process) holds it.
                    // Drop the tx (rollback), sleep, retry.
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
                    .map_err(|e| Error::Other(format!("cache lock insert: {e}")))?;
                    tx.commit()
                        .await
                        .map_err(|e| Error::Other(format!("cache lock commit: {e}")))?;
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

    /// Read of `key`. Caller MUST currently hold the lock for
    /// `key` (via [`lock`] or inside [`get_or_fetch`]).
    pub async fn peek(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, Error> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT body FROM cache WHERE key = ?")
            .bind(&key[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("cache peek: {e}")))
    }

    /// Write of `key` (INSERT OR REPLACE). Triggers LRU eviction
    /// if `max_size > 0` and the post-write total would exceed it.
    pub async fn store(&self, key: &[u8; 32], body: &[u8]) -> Result<(), Error> {
        let now = unix_now();
        sqlx::query("INSERT OR REPLACE INTO cache (key, body, inserted_at) VALUES (?, ?, ?)")
            .bind(&key[..])
            .bind(body)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("cache store: {e}")))?;
        if self.max_size > 0 {
            loop {
                let total: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(SUM(LENGTH(body)), 0) FROM cache",
                )
                .fetch_one(&self.pool)
                .await
                .map_err(|e| Error::Other(format!("cache total: {e}")))?;
                if (total as u64) <= self.max_size {
                    break;
                }
                let removed = sqlx::query(
                    "DELETE FROM cache \
                     WHERE key = (SELECT key FROM cache ORDER BY inserted_at ASC LIMIT 1)",
                )
                .execute(&self.pool)
                .await
                .map_err(|e| Error::Other(format!("cache evict: {e}")))?
                .rows_affected();
                if removed == 0 {
                    // Nothing left to evict but the freshly-inserted
                    // row alone exceeds max_size. Leave it — we
                    // promised to store, eviction is best-effort.
                    break;
                }
            }
        }
        Ok(())
    }

    /// `lock` → `peek` → on miss `fetch` → `store` → release. The
    /// lock is released before this function returns the body, so
    /// the caller never holds the lock while processing the
    /// result. On fetch error the lock is released and the error
    /// propagates — nothing is stored.
    pub async fn get_or_fetch<F, Fut>(
        &self,
        key: &[u8; 32],
        fetch: F,
    ) -> Result<Vec<u8>, Error>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, Error>>,
    {
        let guard = self.lock(key).await?;
        if let Some(body) = self.peek(key).await? {
            guard.release().await;
            return Ok(body);
        }
        let body = match fetch().await {
            Ok(b) => b,
            Err(e) => {
                guard.release().await;
                return Err(e);
            }
        };
        self.store(key, &body).await?;
        guard.release().await;
        Ok(body)
    }
}

/// `SHA-256(method ‖ \0 ‖ path ‖ \0 ‖ query_bytes ‖ \0 ‖ body_bytes)`.
/// `query_bytes` and `body_bytes` are serialized JSON
/// representations (empty when absent). Path params are baked
/// into `path` by the generated endpoint helpers.
pub fn request_key(method: &Method, path: &str, query: &[u8], body: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(method.as_str().as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(query);
    h.update(b"\0");
    h.update(body);
    h.finalize().into()
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

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
