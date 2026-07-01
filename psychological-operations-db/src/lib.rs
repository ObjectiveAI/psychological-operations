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
    /// concurrent queries) so the ~90 processes an `agents queue deliver`
    /// fan-out spawns don't exhaust the shared cluster's connections. Schema
    /// init is gated by [`ensure_schema`] — a FILESYSTEM lock + marker so only
    /// the first co-located process runs the DDL and everyone else skips it
    /// without touching postgres (running the DDL concurrently is what raced the
    /// catalog: `pg_type` duplicate key / `tuple concurrently updated`).
    pub async fn connect(postgres_url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect(postgres_url)
            .await?;
        ensure_schema(&pool, postgres_url).await?;
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

/// Apply the embedded schema exactly once per machine, made safe for concurrent
/// startup with a FILESYSTEM lock + marker (not a postgres lock): every process
/// but the first applier skips init WITHOUT touching postgres at all, so the
/// ~90 processes an `agents queue deliver` fan-out spawns never run the catalog
/// DDL concurrently (which raced: `pg_type` duplicate key / `tuple concurrently
/// updated`). The marker records the schema hash, so a `schema.sql` edit
/// re-applies on the next start.
///
/// Machine-local by design — this deployment runs one postgres per machine, and
/// the marker/lock are keyed by a hash of `postgres_url` (identical across every
/// process sharing one plugin schema, so they coordinate on the same lock).
async fn ensure_schema(pool: &PgPool, postgres_url: &str) -> Result<(), Error> {
    let want = schema_hash();
    let dir = marker_dir(postgres_url);
    let marker = dir.join("schema.applied");

    // Fast path — a marker recording the current schema → skip. No postgres, no
    // lock. This is every process except the single first applier.
    if marker_is_current(&marker, &want) {
        return Ok(());
    }

    // Take the cross-process filesystem lock (blocks until the applier, if any,
    // finishes; crash-safe — the OS frees it if the holder dies). Waiters pin NO
    // postgres connection while blocked here.
    let claim = objectiveai_sdk::lockfile::wait_acquire(
        &dir,
        "schema-init",
        &format!("pid {} schema init", std::process::id()),
    )
    .await
    .map_err(|e| Error::Other(format!("acquire schema lock: {e}")))?;

    let result: Result<(), Error> = async {
        // Re-check under the lock: another process may have applied while we
        // waited on the lock.
        if marker_is_current(&marker, &want) {
            return Ok(());
        }
        // Sole applier: run the DDL, then record the marker so everyone skips.
        sqlx::raw_sql(SCHEMA).execute(pool).await?;
        std::fs::write(&marker, want.as_bytes())
            .map_err(|e| Error::Other(format!("write schema marker: {e}")))?;
        Ok(())
    }
    .await;

    let _ = claim.release();
    result
}

/// Machine-local directory for the schema lock + marker, keyed by a hash of the
/// connection URL (identical across every process sharing one plugin schema, so
/// they coordinate). Under the OS temp dir — always present and identical for
/// every co-located process, unlike the per-command env.
fn marker_dir(postgres_url: &str) -> std::path::PathBuf {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(postgres_url.as_bytes());
    std::env::temp_dir()
        .join("objectiveai-psyop-schema")
        .join(format!("{:016x}", h.finish()))
}

/// True iff the marker exists and records the current schema hash. A missing
/// marker OR a stale hash (schema.sql changed) both mean "must (re)apply".
fn marker_is_current(marker: &std::path::Path, want: &str) -> bool {
    std::fs::read_to_string(marker)
        .map(|s| s.trim() == want)
        .unwrap_or(false)
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
