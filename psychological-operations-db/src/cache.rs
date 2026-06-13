//! X v2 API response cache. Bodies live in the `cache` table keyed by
//! the caller-computed `SHA-256(method ‖ path ‖ query ‖ body)` (the key
//! helpers stay in the SDK — they need `reqwest::Method`). Eviction is
//! LRU by `inserted_at`, capped at `max_size` bytes (0 disables). Reads
//! drop rows older than `inserted_at + ttl`.

use std::time::Duration;

use crate::{Db, Error, unix_now};

impl Db {
    /// `lock` → `peek` → on miss `fetch` → `store` → release. The
    /// per-key advisory lock collapses concurrent identical requests
    /// (thundering-herd guard); cache hits return before `fetch` runs.
    ///
    /// Generic over the caller's error `E` so the fetch closure can
    /// surface its own error type (e.g. the SDK's transport/quota
    /// errors); the locker/peek/store steps' [`Error`] converts into
    /// `E` via `From`.
    pub async fn cache_get_or_fetch<F, Fut, E>(
        &self,
        key: &[u8; 32],
        max_size: u64,
        ttl: Duration,
        fetch: F,
    ) -> Result<Vec<u8>, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, E>>,
        E: From<Error>,
    {
        let guard = self.lock(key).await?;
        if let Some(body) = self.cache_peek(key, ttl).await? {
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
        self.cache_store(key, &body, max_size).await?;
        guard.release().await;
        Ok(body)
    }

    /// Read `key`, treating rows older than `inserted_at + ttl` as
    /// misses. Caller holds the per-key lock (via `cache_get_or_fetch`).
    async fn cache_peek(&self, key: &[u8; 32], ttl: Duration) -> Result<Option<Vec<u8>>, Error> {
        let ttl_secs = ttl.as_secs().min(i64::MAX as u64) as i64;
        let cutoff = unix_now().saturating_sub(ttl_secs);
        let out: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT body FROM cache WHERE key = $1 AND inserted_at >= $2")
                .bind(&key[..])
                .bind(cutoff)
                .fetch_optional(&self.pool)
                .await?;
        Ok(out)
    }

    /// Upsert `key`, then LRU-evict until total body bytes ≤ `max_size`
    /// (when `max_size > 0`).
    async fn cache_store(&self, key: &[u8; 32], body: &[u8], max_size: u64) -> Result<(), Error> {
        let now = unix_now();
        sqlx::query(
            "INSERT INTO cache (key, body, inserted_at) VALUES ($1, $2, $3)
             ON CONFLICT (key) DO UPDATE SET body = excluded.body, inserted_at = excluded.inserted_at",
        )
        .bind(&key[..])
        .bind(body)
        .bind(now)
        .execute(&self.pool)
        .await?;
        if max_size > 0 {
            loop {
                let total: i64 =
                    sqlx::query_scalar("SELECT COALESCE(SUM(LENGTH(body)), 0)::bigint FROM cache")
                        .fetch_one(&self.pool)
                        .await?;
                if (total as u64) <= max_size {
                    break;
                }
                let removed = sqlx::query(
                    "DELETE FROM cache
                     WHERE key = (SELECT key FROM cache ORDER BY inserted_at ASC LIMIT 1)",
                )
                .execute(&self.pool)
                .await?
                .rows_affected();
                if removed == 0 {
                    break;
                }
            }
        }
        Ok(())
    }
}
