//! Per-(agent, target) engagement record.
//!
//! Tweet-scoped engagements (`replies`, `retweets`, `likes`,
//! `quotes`) live in four parallel tables, each keyed by
//! `(tweet_id, agent)`. A fifth table `follows` records
//! user-scoped engagements keyed by `(user_id, agent)`. All five
//! are additive: a row's presence means "this agent performed
//! this engagement at some point."
//!
//! [`Engagement::get`] joins the four tweet tables in a single
//! query and returns the bool-struct. [`Engagement::is_following`]
//! is a separate single-row probe (follows aren't per-tweet so it
//! doesn't make sense to bundle into the same struct). Each
//! `mark_*` is an `INSERT OR IGNORE` into its own table.
//!
//! Storage: `<config_base_dir>/plugins/psychological-operations/x-api-mcp.sqlite`,
//! sibling to `x-api-cache.sqlite` and `queue.sqlite`. WAL + 5 s
//! busy timeout, schema versioned via the same `schema_version`
//! pattern queue.rs uses.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use super::super::Error;
use super::super::locker;

const REPLIES_VERSION:  i64 = 1;
const RETWEETS_VERSION: i64 = 1;
const LIKES_VERSION:    i64 = 1;
const QUOTES_VERSION:   i64 = 1;
const FOLLOWS_VERSION:  i64 = 1;

const REPLIES_CREATE: &str = "CREATE TABLE replies (\
        tweet_id   TEXT    NOT NULL,\
        agent      TEXT    NOT NULL,\
        created_at INTEGER NOT NULL,\
        PRIMARY KEY (tweet_id, agent)\
    )";

const RETWEETS_CREATE: &str = "CREATE TABLE retweets (\
        tweet_id   TEXT    NOT NULL,\
        agent      TEXT    NOT NULL,\
        created_at INTEGER NOT NULL,\
        PRIMARY KEY (tweet_id, agent)\
    )";

const LIKES_CREATE: &str = "CREATE TABLE likes (\
        tweet_id   TEXT    NOT NULL,\
        agent      TEXT    NOT NULL,\
        created_at INTEGER NOT NULL,\
        PRIMARY KEY (tweet_id, agent)\
    )";

const QUOTES_CREATE: &str = "CREATE TABLE quotes (\
        tweet_id   TEXT    NOT NULL,\
        agent      TEXT    NOT NULL,\
        created_at INTEGER NOT NULL,\
        PRIMARY KEY (tweet_id, agent)\
    )";

const FOLLOWS_CREATE: &str = "CREATE TABLE follows (\
        user_id    TEXT    NOT NULL,\
        agent      TEXT    NOT NULL,\
        created_at INTEGER NOT NULL,\
        PRIMARY KEY (user_id, agent)\
    )";

/// Whether `(agent, tweet_id)` has been recorded in each of the
/// four engagement tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Engagement {
    pub replied: bool,
    pub retweeted: bool,
    pub liked: bool,
    pub quoted: bool,
}

/// SQLite-backed `(agent, tweet_id)` engagement store. Open via
/// [`EngagementStore::open`] or — preferred — through
/// `Client::engagement()` (lazy `OnceCell`).
pub struct EngagementStore {
    pool: SqlitePool,
}

impl std::fmt::Debug for EngagementStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngagementStore").finish_non_exhaustive()
    }
}

impl EngagementStore {
    /// Open (creating if missing) the engagement file under
    /// `<config_base_dir>/plugins/psychological-operations/x-api-mcp.sqlite`.
    /// Enables WAL + a 5 s busy timeout. Creates / upgrades all
    /// four tables on first open.
    pub async fn open(config_base_dir: &Path) -> Result<Self, Error> {
        let pool = open_pool(config_base_dir).await?;
        Self::ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_version (\
                 name    TEXT    PRIMARY KEY,\
                 version INTEGER NOT NULL\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("engagement schema_version: {e}")))?;

        ensure_table(pool, "replies",  REPLIES_VERSION,  REPLIES_CREATE).await?;
        ensure_table(pool, "retweets", RETWEETS_VERSION, RETWEETS_CREATE).await?;
        ensure_table(pool, "likes",    LIKES_VERSION,    LIKES_CREATE).await?;
        ensure_table(pool, "quotes",   QUOTES_VERSION,   QUOTES_CREATE).await?;
        ensure_table(pool, "follows",  FOLLOWS_VERSION,  FOLLOWS_CREATE).await?;
        Ok(())
    }

    /// Combined existence check across all four tables in a
    /// single round-trip.
    pub async fn get(
        &self,
        agent: &str,
        tweet_id: &str,
    ) -> Result<Engagement, Error> {
        let row: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT \
                EXISTS(SELECT 1 FROM replies   WHERE tweet_id = ?1 AND agent = ?2) AS replied, \
                EXISTS(SELECT 1 FROM retweets  WHERE tweet_id = ?1 AND agent = ?2) AS retweeted, \
                EXISTS(SELECT 1 FROM likes     WHERE tweet_id = ?1 AND agent = ?2) AS liked, \
                EXISTS(SELECT 1 FROM quotes    WHERE tweet_id = ?1 AND agent = ?2) AS quoted",
        )
        .bind(tweet_id)
        .bind(agent)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("engagement get: {e}")))?;
        Ok(Engagement {
            replied:   row.0 != 0,
            retweeted: row.1 != 0,
            liked:     row.2 != 0,
            quoted:    row.3 != 0,
        })
    }

    /// `INSERT OR IGNORE INTO replies   (tweet_id, agent, created_at)`.
    pub async fn mark_replied(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.insert("replies", "tweet_id", agent, tweet_id).await
    }

    /// `INSERT OR IGNORE INTO retweets  (tweet_id, agent, created_at)`.
    pub async fn mark_retweeted(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.insert("retweets", "tweet_id", agent, tweet_id).await
    }

    /// `INSERT OR IGNORE INTO likes     (tweet_id, agent, created_at)`.
    pub async fn mark_liked(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.insert("likes", "tweet_id", agent, tweet_id).await
    }

    /// `INSERT OR IGNORE INTO quotes    (tweet_id, agent, created_at)`.
    pub async fn mark_quoted(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.insert("quotes", "tweet_id", agent, tweet_id).await
    }

    /// Whether `(user_id, agent)` is recorded in the `follows`
    /// table. Single-row probe — follows aren't bundled into the
    /// per-tweet [`Engagement`] struct because they're keyed by
    /// X user id, not tweet id.
    pub async fn is_following(
        &self,
        agent: &str,
        user_id: &str,
    ) -> Result<bool, Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM follows WHERE user_id = ? AND agent = ?) AS following",
        )
        .bind(user_id)
        .bind(agent)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("engagement is_following: {e}")))?;
        Ok(row.0 != 0)
    }

    /// `INSERT OR IGNORE INTO follows   (user_id, agent, created_at)`.
    pub async fn mark_followed(&self, agent: &str, user_id: &str) -> Result<(), Error> {
        self.insert("follows", "user_id", agent, user_id).await
    }

    async fn insert(
        &self,
        table: &str,
        key_col: &str,
        agent: &str,
        key_value: &str,
    ) -> Result<(), Error> {
        let sql = format!(
            "INSERT OR IGNORE INTO {table} ({key_col}, agent, created_at) VALUES (?, ?, ?)"
        );
        sqlx::query(&sql)
            .bind(key_value)
            .bind(agent)
            .bind(locker::unix_now())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("engagement {table} insert: {e}")))?;
        Ok(())
    }
}

/// Idempotent table create/upgrade. Reads the recorded version from
/// `schema_version` and, on missing-or-mismatched, drops the existing
/// table + runs `create_stmt` + stamps the new version. No-op when the
/// recorded version matches. Mirrors `queue.rs::ensure_table`.
async fn ensure_table(
    pool: &SqlitePool,
    name: &str,
    version: i64,
    create_stmt: &str,
) -> Result<(), Error> {
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT version FROM schema_version WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Other(format!("{name} version read: {e}")))?;

    if current == Some(version) {
        return Ok(());
    }

    sqlx::query(&format!("DROP TABLE IF EXISTS {name}"))
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("{name} drop: {e}")))?;
    sqlx::query(create_stmt)
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("{name} create: {e}")))?;
    sqlx::query(
        "INSERT OR REPLACE INTO schema_version (name, version) VALUES (?, ?)",
    )
    .bind(name)
    .bind(version)
    .execute(pool)
    .await
    .map_err(|e| Error::Other(format!("{name} version stamp: {e}")))?;
    Ok(())
}

/// Open the engagement's SQLite file (a sibling of
/// `x-api-cache.sqlite` and `queue.sqlite` under
/// `<config>/plugins/psychological-operations/`).
async fn open_pool(config_base_dir: &Path) -> Result<SqlitePool, Error> {
    let dir = config_base_dir
        .join("plugins")
        .join("psychological-operations");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(format!("engagement mkdir: {e}")))?;
    let path = dir.join("x-api-mcp.sqlite");

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(100)
        .connect_with(opts)
        .await
        .map_err(|e| Error::Other(format!("engagement pool open {}: {e}", path.display())))
}
