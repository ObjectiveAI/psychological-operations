//! Per-agent tweet handling queue.
//!
//! Rows are keyed by `(agent, tweet_id)`. Two sources write to it:
//!
//! - **Psyop pipelines** — a tweet that scored above its
//!   threshold lands here with `psyop = Some(name)` + `score =
//!   Some(value)` + no `deliverer` / `message`.
//! - **`agents enqueue`** — an operator flags a tweet for the
//!   current agent with `deliverer = Some(agent)` + `message =
//!   Some(note)` + no `psyop` / `score`.
//!
//! Agent-side, the `read_queue` MCP tool lists pending entries
//! and `mark_handled` removes one.
//!
//! Storage: `<config_base_dir>/plugins/psychological-operations/queue.sqlite`,
//! a separate file from the response cache. WAL + 5 s busy
//! timeout so concurrent processes don't fail with `SQLITE_BUSY`.
//!
//! Re-enqueueing the same `(agent, tweet_id)` overwrites the row
//! (`INSERT OR REPLACE`); the other columns are "the rest is
//! irrelevant" per the design.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use super::Error;
use super::locker;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub agent: String,
    pub tweet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psyop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub queued_at: i64,
}

/// SQLite-backed per-agent tweet queue. Open via [`Queue::open`]
/// or — preferred — through `Client::queue()` (lazy `OnceCell`).
pub struct Queue {
    pool: SqlitePool,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue").finish_non_exhaustive()
    }
}

impl Queue {
    /// Open (creating if missing) the queue file under
    /// `<config_base_dir>/plugins/psychological-operations/queue.sqlite`.
    /// Enables WAL + a 5 s busy timeout. Creates the `queue` table
    /// on first open.
    pub async fn open(config_base_dir: &Path) -> Result<Self, Error> {
        let pool = open_pool(config_base_dir).await?;
        Self::ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS queue (\
                 agent       TEXT    NOT NULL,\
                 tweet_id    TEXT    NOT NULL,\
                 psyop       TEXT,\
                 score       REAL,\
                 deliverer   TEXT,\
                 message     TEXT,\
                 queued_at   INTEGER NOT NULL,\
                 PRIMARY KEY (agent, tweet_id)\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("queue schema: {e}")))?;
        Ok(())
    }

    /// Upsert by `(agent, tweet_id)`. Re-enqueueing overwrites the
    /// other columns wholesale.
    pub async fn enqueue(&self, entry: &QueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT OR REPLACE INTO queue \
             (agent, tweet_id, psyop, score, deliverer, message, queued_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.agent)
        .bind(&entry.tweet_id)
        .bind(entry.psyop.as_deref())
        .bind(entry.score)
        .bind(entry.deliverer.as_deref())
        .bind(entry.message.as_deref())
        .bind(entry.queued_at)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("queue enqueue: {e}")))?;
        Ok(())
    }

    /// All entries for `agent`, oldest first.
    pub async fn list(&self, agent: &str) -> Result<Vec<QueueEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent, tweet_id, psyop, score, deliverer, message, queued_at \
             FROM queue \
             WHERE agent = ? \
             ORDER BY queued_at ASC",
        )
        .bind(agent)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("queue list: {e}")))?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Delete `(agent, tweet_id)` if present. Returns `true` if a
    /// row was removed.
    pub async fn delete(&self, agent: &str, tweet_id: &str) -> Result<bool, Error> {
        let result = sqlx::query("DELETE FROM queue WHERE agent = ? AND tweet_id = ?")
            .bind(agent)
            .bind(tweet_id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("queue delete: {e}")))?;
        Ok(result.rows_affected() > 0)
    }
}

fn row_to_entry(row: sqlx::sqlite::SqliteRow) -> QueueEntry {
    QueueEntry {
        agent:     row.get("agent"),
        tweet_id:  row.get("tweet_id"),
        psyop:     row.try_get("psyop").ok(),
        score:     row.try_get("score").ok(),
        deliverer: row.try_get("deliverer").ok(),
        message:   row.try_get("message").ok(),
        queued_at: row.get("queued_at"),
    }
}

/// Build the `unix_now`-stamped current time the way other SDK
/// modules do. Re-exported so callers can construct
/// `QueueEntry.queued_at` without a separate `time` dep.
pub fn unix_now() -> i64 {
    locker::unix_now()
}

/// Open the queue's SQLite file (a sibling of `x-api-cache.sqlite`
/// under `<config>/plugins/psychological-operations/`).
async fn open_pool(config_base_dir: &Path) -> Result<SqlitePool, Error> {
    let dir = config_base_dir
        .join("plugins")
        .join("psychological-operations");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(format!("queue mkdir: {e}")))?;
    let path = dir.join("queue.sqlite");

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(100)
        .connect_with(opts)
        .await
        .map_err(|e| Error::Other(format!("queue pool open {}: {e}", path.display())))
}
