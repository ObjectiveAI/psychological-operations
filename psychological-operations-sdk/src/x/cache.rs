//! Cross-process SQLite-backed response cache for the X v2 API
//! client. Stores raw 2xx response bodies keyed by
//! `SHA-256(method ‖ path ‖ query ‖ body)`.
//!
//! Per-key exclusive mutex (not RWLock) — at most one holder per
//! key at any time. Different keys lock independently, so
//! unrelated requests still run in parallel. Cross-process via a
//! `locks` table: a 16-byte random `holder_id` per acquire, a 10 s
//! `expires_at` TTL, and a 1 s refresher task that re-stamps the
//! TTL while the holder is alive. A SIGKILL'd holder's row ages
//! out within 10 s and the next acquire's GC sweeps it.
//!
//! Two API layers:
//!
//!   * [`Cache::get_or_fetch`] — convenience. Lock → peek →
//!     fetch-if-needed → store → release-before-return.
//!   * [`Cache::lock`] / [`Cache::peek`] / [`Cache::store`] — the
//!     primitives `get_or_fetch` is built from, exposed so a
//!     caller that wants to release the lock the instant it has
//!     the body (e.g. before doing slow post-processing) can
//!     orchestrate the cycle by hand.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use reqwest::Method;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use super::Error;

/// Cross-process per-key mutex TTL. Refreshed every 1 s by the
/// holder's spawned task; aged out by the next acquire's GC if
/// the holder dies.
const LOCK_TTL_SECS: i64 = 10;
const LOCK_REFRESH_SECS: u64 = 1;
const LOCK_POLL_INTERVAL_MS: u64 = 50;

/// SQLite-backed response cache. One file per
/// `<config-base-dir>/plugins/psychological-operations/x-api-cache.sqlite`.
pub struct Cache {
    conn: Arc<Mutex<Connection>>,
    /// Bytes. When > 0, [`store`] evicts oldest entries
    /// (by `inserted_at`) until total body size ≤ this value.
    /// 0 disables eviction.
    max_size: u64,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("max_size", &self.max_size)
            .finish_non_exhaustive()
    }
}

/// Held lock on one key. Drop releases (kills the refresher task
/// + deletes the row); call [`release`] to drop inline inside an
/// `async` function without juggling scopes.
pub struct LockGuard {
    conn: Arc<Mutex<Connection>>,
    key: [u8; 32],
    holder_id: [u8; 16],
    refresher: Option<JoinHandle<()>>,
}

impl LockGuard {
    /// Drop the lock now. `drop(guard)` does the same thing — this
    /// just reads inline.
    pub fn release(self) {
        // Drop runs on scope exit (immediately).
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(h) = self.refresher.take() {
            h.abort();
        }
        if let Ok(c) = self.conn.lock() {
            let _ = c.execute(
                "DELETE FROM locks WHERE key = ? AND holder_id = ?",
                params![&self.key[..], &self.holder_id[..]],
            );
        }
        // Failure to delete is harmless — the row's TTL expires
        // and the next acquire's GC sweeps it.
    }
}

impl Cache {
    /// Open (creating if missing) the cache file under
    /// `<config_base_dir>/plugins/psychological-operations/x-api-cache.sqlite`.
    /// Enables WAL + a 5 s busy timeout so concurrent processes
    /// don't fail with `SQLITE_BUSY` on contention.
    pub fn open(config_base_dir: &Path, max_size: u64) -> Result<Self, Error> {
        let dir = config_base_dir
            .join("plugins")
            .join("psychological-operations");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Other(format!("cache mkdir: {e}")))?;
        let path = dir.join("x-api-cache.sqlite");
        let conn = Connection::open(&path)
            .map_err(|e| Error::Other(format!("cache open {}: {e}", path.display())))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| Error::Other(format!("cache busy_timeout: {e}")))?;
        conn.pragma_update(None, "journal_mode", &"WAL")
            .map_err(|e| Error::Other(format!("cache PRAGMA journal_mode: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 body BLOB NOT NULL,\
                 inserted_at INTEGER NOT NULL\
             );\
             CREATE TABLE IF NOT EXISTS locks (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 holder_id BLOB NOT NULL,\
                 expires_at INTEGER NOT NULL\
             );",
        )
        .map_err(|e| Error::Other(format!("cache schema: {e}")))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            max_size,
        })
    }

    /// Acquire the per-key mutex. Polls every 50 ms until the
    /// previous holder releases (or its TTL expires). Returns a
    /// guard that releases on drop or via [`LockGuard::release`].
    pub async fn lock(&self, key: &[u8; 32]) -> Result<LockGuard, Error> {
        let mut holder_id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut holder_id);

        loop {
            let acquired = {
                let mut conn = self.conn.lock().map_err(poisoned)?;
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| Error::Other(format!("cache lock tx: {e}")))?;
                let now = unix_now();
                tx.execute(
                    "DELETE FROM locks WHERE key = ? AND expires_at <= ?",
                    params![&key[..], now],
                )
                .map_err(|e| Error::Other(format!("cache lock gc: {e}")))?;
                let live: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM locks WHERE key = ?",
                        params![&key[..]],
                        |row| row.get(0),
                    )
                    .map_err(|e| Error::Other(format!("cache lock probe: {e}")))?;
                if live > 0 {
                    // Someone else holds the key. Rollback the tx
                    // (drop also rolls back), drop the connection
                    // lock, sleep before retrying so we don't
                    // spin under the connection mutex.
                    drop(tx);
                    false
                } else {
                    tx.execute(
                        "INSERT INTO locks (key, holder_id, expires_at) VALUES (?, ?, ?)",
                        params![&key[..], &holder_id[..], now + LOCK_TTL_SECS],
                    )
                    .map_err(|e| Error::Other(format!("cache lock insert: {e}")))?;
                    tx.commit()
                        .map_err(|e| Error::Other(format!("cache lock commit: {e}")))?;
                    true
                }
            };
            if acquired {
                let refresher = spawn_refresher(self.conn.clone(), *key, holder_id);
                return Ok(LockGuard {
                    conn: self.conn.clone(),
                    key: *key,
                    holder_id,
                    refresher: Some(refresher),
                });
            }
            tokio::time::sleep(Duration::from_millis(LOCK_POLL_INTERVAL_MS)).await;
        }
    }

    /// Locked read of `key`. Caller MUST currently hold the lock
    /// for `key` (via [`lock`] or inside [`get_or_fetch`]).
    pub fn peek(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, Error> {
        let conn = self.conn.lock().map_err(poisoned)?;
        conn.query_row(
            "SELECT body FROM cache WHERE key = ?",
            params![&key[..]],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|e| Error::Other(format!("cache peek: {e}")))
    }

    /// Locked write of `key` (INSERT OR REPLACE). Triggers LRU
    /// eviction if `max_size > 0` and the post-write total would
    /// exceed it.
    pub fn store(&self, key: &[u8; 32], body: &[u8]) -> Result<(), Error> {
        let conn = self.conn.lock().map_err(poisoned)?;
        let now = unix_now();
        conn.execute(
            "INSERT OR REPLACE INTO cache (key, body, inserted_at) VALUES (?, ?, ?)",
            params![&key[..], body, now],
        )
        .map_err(|e| Error::Other(format!("cache store: {e}")))?;
        if self.max_size > 0 {
            loop {
                let total: i64 = conn
                    .query_row(
                        "SELECT COALESCE(SUM(LENGTH(body)), 0) FROM cache",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|e| Error::Other(format!("cache total: {e}")))?;
                if (total as u64) <= self.max_size {
                    break;
                }
                let removed = conn
                    .execute(
                        "DELETE FROM cache \
                         WHERE key = (SELECT key FROM cache ORDER BY inserted_at ASC LIMIT 1)",
                        [],
                    )
                    .map_err(|e| Error::Other(format!("cache evict: {e}")))?;
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
    /// lock is dropped before this function returns the body, so
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
        if let Some(body) = self.peek(key)? {
            guard.release();
            return Ok(body);
        }
        let body = match fetch().await {
            Ok(b) => b,
            Err(e) => {
                guard.release();
                return Err(e);
            }
        };
        self.store(key, &body)?;
        guard.release();
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
    conn: Arc<Mutex<Connection>>,
    key: [u8; 32],
    holder_id: [u8; 16],
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(LOCK_REFRESH_SECS)).await;
            let now = unix_now();
            if let Ok(c) = conn.lock() {
                let _ = c.execute(
                    "UPDATE locks SET expires_at = ? \
                     WHERE key = ? AND holder_id = ?",
                    params![now + LOCK_TTL_SECS, &key[..], &holder_id[..]],
                );
            }
        }
    })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn poisoned<T>(_: T) -> Error {
    Error::Other("cache connection mutex poisoned".into())
}
