//! The single persistence layer for psychological-operations.
//!
//! Everything our system stores lives in postgres, reached through the
//! cloneable [`Db`] handle (one connection pool per process). This is
//! the only crate that depends on `sqlx`. The lone exception to
//! "postgres for everything" is [`cookies`] — a read-only probe of
//! **Chromium's** own on-disk SQLite cookie jar (it isn't ours to move
//! to postgres), which is why the crate also enables sqlx's `sqlite`
//! feature.
//!
//! Storage-only by design: methods take/return JSON
//! ([`serde_json::Value`]), primitives, byte slices, and the small row
//! DTOs in this crate. The domain types (`PsyOp`, `Tokens`) live in
//! the SDK/CLI and are (de)serialized at the call sites — the db crate
//! never depends on them.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use sqlx::postgres::{PgPool, PgPoolOptions};

pub mod actions;
pub mod auth;
pub mod cache;
pub mod delivered;
pub mod cookies;
pub mod discord;
pub mod discord_hooks;
pub mod discord_queue;
pub mod locker;
pub mod posts;
pub mod psyops;
pub mod quota;
pub mod reload;
pub mod reply_queue;
pub mod retry;
pub mod twitch;
pub mod twitch_hooks;
pub mod twitch_queue;
pub mod x_app;
pub mod x_queue;

pub use cookies::{parse_twid, signed_in_x_user_id, CookiesError};
pub use discord::DiscordAuth;
pub use discord_hooks::DiscordHookEntry;
pub use discord_queue::DiscordQueueEntry;
pub use locker::LockGuard;
pub use posts::{MediaUrl, Origin, Post};
pub use reload::ReloadListener;
pub use twitch::{TwitchApp, TwitchAuth, TwitchMessage};
pub use twitch_hooks::TwitchHookEntry;
pub use twitch_queue::TwitchQueueEntry;
pub use x_queue::XQueueEntry;
pub use reply_queue::ReplyQuoteEntry;

/// The embedded schema, applied idempotently on [`Db::connect`].
const SCHEMA: &str = include_str!("schema.sql");

/// Dedicated `pg_advisory_lock` key that serializes schema initialization
/// across every process sharing this postgres (the ASCII bytes `"psyopsch"`).
/// A fixed constant in the advisory-lock bigint keyspace; a collision with a
/// runtime [`locker`] key would only cause harmless extra serialization during
/// init. Serializing is what makes the otherwise-racy catalog DDL
/// (`CREATE TABLE IF NOT EXISTS`, `DROP/CREATE TRIGGER`) safe when many agents
/// start at once.
const SCHEMA_LOCK_KEY: i64 = 0x7073_796f_7073_6368u64 as i64;

/// Failure modes of any db operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Chromium cookie probe failure (I/O, decrypt, key material).
    #[error("cookie: {0}")]
    Cookie(#[from] cookies::CookiesError),
    #[error("{0}")]
    Other(String),
}

/// Cloneable handle to the postgres pool (+ the in-process tier of the
/// advisory locker). Clone is cheap — the pool is `Arc` internally.
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
    /// Tier-1 of the locker: per-key in-process mutex, so same-process
    /// contenders never round-trip to postgres. Tier-2 is a postgres
    /// advisory lock — see [`locker`].
    inproc_locks: Arc<DashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db {
    /// Open the pool against `postgres_url` and apply the schema.
    ///
    /// The pool is small (a plugin/MCP process issues only a handful of
    /// concurrent queries) so that the ~90 processes an `agents queue deliver`
    /// fan-out spawns don't exhaust the shared cluster's connections. Schema
    /// init is serialized + skipped-when-current by [`ensure_schema`] — running
    /// the full DDL concurrently is what previously raced the catalog
    /// (`pg_type` duplicate key / `tuple concurrently updated`).
    pub async fn connect(postgres_url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect(postgres_url)
            .await?;
        ensure_schema(&pool).await?;
        Ok(Self {
            pool,
            inproc_locks: Arc::new(DashMap::new()),
        })
    }

    /// Escape hatch for callers that need the raw pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// Apply the embedded schema, made safe for concurrent startup. Only one
/// process runs the DDL at a time (a dedicated advisory lock), and it's skipped
/// entirely once a `schema_meta` row records the current schema hash — so a
/// stampede of agent starts does a ~1ms lock + `SELECT` each rather than racing
/// the catalog. A `schema.sql` edit changes the hash, so upgrades still
/// auto-apply on the next connect.
async fn ensure_schema(pool: &PgPool) -> Result<(), Error> {
    let want = schema_hash();

    // Acquire the init lock with try + backoff, RELEASING the connection
    // between attempts so waiters don't pin a pool/cluster connection while the
    // single applier runs the DDL. The lock is guaranteed to free (the holder
    // releases it, or its session dies and postgres frees it), so this loop
    // always resolves.
    loop {
        let mut conn = pool.acquire().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(SCHEMA_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await?;
        if locked {
            let result = apply_schema_if_stale(&mut conn, &want).await;
            // Always release the session lock before the connection returns to
            // the pool (a pooled session is reused, not closed).
            let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(SCHEMA_LOCK_KEY)
                .execute(&mut *conn)
                .await;
            return result;
        }
        drop(conn);
        // Jitter off the pid so contenders don't retry in lockstep (no rand dep
        // here).
        let backoff = 20 + (std::process::id() as u64 % 60);
        tokio::time::sleep(Duration::from_millis(backoff)).await;
    }
}

/// Under the init lock: create the sentinel table, then run the schema DDL only
/// if the stored hash doesn't match the embedded schema (first boot, or the
/// first process after a `schema.sql` edit). Cheap no-op on the warm path.
async fn apply_schema_if_stale(
    conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    want: &str,
) -> Result<(), Error> {
    // Safe to `CREATE TABLE IF NOT EXISTS` here without racing the catalog:
    // this runs under the exclusive advisory lock.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_meta (\
             only_row boolean PRIMARY KEY DEFAULT true, \
             hash     text NOT NULL)",
    )
    .execute(&mut **conn)
    .await?;

    let have: Option<String> = sqlx::query_scalar("SELECT hash FROM schema_meta WHERE only_row")
        .fetch_optional(&mut **conn)
        .await?;
    if have.as_deref() == Some(want) {
        return Ok(()); // schema already current — skip the DDL entirely.
    }

    sqlx::raw_sql(SCHEMA).execute(&mut **conn).await?;
    sqlx::query(
        "INSERT INTO schema_meta (only_row, hash) VALUES (true, $1) \
         ON CONFLICT (only_row) DO UPDATE SET hash = excluded.hash",
    )
    .bind(want)
    .execute(&mut **conn)
    .await?;
    Ok(())
}

/// Deterministic hash of the embedded schema — stable across processes of the
/// same binary (so every concurrent starter computes the same value) and
/// changing whenever `schema.sql` is edited. Not cryptographic; only used to
/// detect "same schema as last applied".
fn schema_hash() -> String {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(SCHEMA.as_bytes());
    format!("{:016x}", h.finish())
}

/// Unix seconds — shared by every store that timestamps with a
/// `BIGINT` column (cache, queue, request log, psyop_runs).
pub fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
