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
    /// Open the pool against `postgres_url` and apply the schema. The
    /// schema is all `CREATE … IF NOT EXISTS`, so this is idempotent.
    pub async fn connect(postgres_url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(100)
            .connect(postgres_url)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
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

/// Unix seconds — shared by every store that timestamps with a
/// `BIGINT` column (cache, queue, request log, psyop_runs).
pub fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
