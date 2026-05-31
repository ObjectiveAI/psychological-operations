//! SQLite-backed response cache for the X v2 API client.
//!
//! Bodies live in the `cache` table keyed by
//! `SHA-256(method ‖ path ‖ query ‖ body)`. Eviction is LRU by
//! `inserted_at`, capped at `max_size` bytes (0 disables).
//!
//! Two-tier per-key locking lives in [`super::locker::Locker`];
//! `Cache` composes one. See that module for the cross-process
//! algorithm + the release-ordering invariant. Cache and auth
//! share a single `SqlitePool` (and hence the `locks` table),
//! but each owns its own `Locker` instance — independent in-process
//! key spaces.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use super::Error;
use super::locker::{self, Locker, LockGuard};

/// SQLite-backed response cache. Pool is shared with the auth
/// locker; `Cache::open` is the entry point that creates the pool
/// (and both `cache`/`locks` tables) for the whole SDK.
pub struct Cache {
    pool: SqlitePool,
    locker: Locker,
    /// Bytes. When > 0, [`store`] evicts oldest entries (by
    /// `inserted_at`) until total body size ≤ this value. 0
    /// disables eviction.
    max_size: u64,
    /// Per-entry TTL. Plumbed through the SDK constructors but
    /// NOT yet consumed by the eviction logic — eviction is still
    /// pure LRU-by-`inserted_at` capped at `max_size`. A future
    /// change will use this to skip-or-evict entries older than
    /// `inserted_at + cache_ttl`. Stored on the struct now so the
    /// constructor signatures + downstream binaries can settle
    /// before the eviction work lands.
    #[allow(dead_code)]
    cache_ttl: Duration,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("max_size", &self.max_size)
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// Open (creating if missing) the cache file under
    /// `<config_base_dir>/plugins/psychological-operations/x-api-cache.sqlite`.
    /// Enables WAL + a 5 s busy timeout so concurrent processes
    /// don't fail with `SQLITE_BUSY` on contention. Creates both
    /// the `cache` and `locks` tables.
    pub async fn open(
        config_base_dir: &Path,
        max_size: u64,
        cache_ttl: Duration,
    ) -> Result<Self, Error> {
        let pool = open_pool(config_base_dir).await?;
        locker::Locker::ensure_schema(&pool).await?;
        Self::ensure_schema(&pool).await?;
        Ok(Self {
            locker: Locker::new(pool.clone()),
            pool,
            max_size,
            cache_ttl,
        })
    }

    /// Idempotent `cache` table creation. Public to the crate so
    /// `Http` can wire shared-pool callers without re-running
    /// `Cache::open`.
    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cache (\
                 key BLOB PRIMARY KEY NOT NULL,\
                 body BLOB NOT NULL,\
                 inserted_at INTEGER NOT NULL\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("cache schema: {e}")))?;
        Ok(())
    }

    /// Acquire the per-key lock via the inner Locker.
    pub async fn lock(&self, key: &[u8; 32]) -> Result<LockGuard, Error> {
        self.locker.acquire(key).await
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
        let now = locker::unix_now();
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
                    break;
                }
            }
        }
        Ok(())
    }

    /// `lock` → `peek` → on miss `fetch` → `store` → release.
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

    /// Expose the underlying pool so other concerns sharing this
    /// SQLite file (e.g. the auth locker) can be constructed
    /// against it.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// `SHA-256("cache\0" ‖ method ‖ \0 ‖ path ‖ \0 ‖ query ‖ \0 ‖ body)`.
/// The `cache\0` prefix namespaces away from the auth locker's
/// keys (`"auth\0" ‖ …`) so they can share the `locks` table
/// without ever colliding.
pub fn request_key(method: &Method, path: &str, query: &[u8], body: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"cache\0");
    h.update(method.as_str().as_bytes());
    h.update(b"\0");
    h.update(path.as_bytes());
    h.update(b"\0");
    h.update(query);
    h.update(b"\0");
    h.update(body);
    h.finalize().into()
}

/// Open the SDK's SQLite file (`<config>/plugins/psychological-operations/x-api-cache.sqlite`).
/// Both the cache and the auth locker share this one file/pool.
pub(crate) async fn open_pool(config_base_dir: &Path) -> Result<SqlitePool, Error> {
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

    SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .map_err(|e| Error::Other(format!("cache pool open {}: {e}", path.display())))
}

/// Build an `Arc<Cache>` for a given config root (only when
/// `max_size > 0`). Used by `Http::{app_only, for_psyop}` so they
/// don't each re-implement the cache-or-not branch.
pub(crate) async fn open_optional(
    config_base_dir: &Path,
    max_size: u64,
    cache_ttl: Duration,
) -> Result<Option<Arc<Cache>>, Error> {
    if max_size == 0 {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        Cache::open(config_base_dir, max_size, cache_ttl).await?,
    )))
}
