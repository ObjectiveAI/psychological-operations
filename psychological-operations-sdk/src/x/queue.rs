//! Per-agent tweet handling queue.
//!
//! Queue rows are keyed by `(agent, tweet_id)` — the queue is a
//! shared "to-do list" for each X-API persona, **not** partitioned
//! by operator. Any operator's handler agent reading alice's queue
//! sees the same rows. Two sources write:
//!
//! - **Psyop pipelines** — a tweet that scored above its threshold
//!   lands here with `psyop = Some(name)` + `score = Some(value)` +
//!   no `deliverer_agent_instance_hierarchy` / `message`.
//! - **`agents enqueue`** — an operator flags a tweet for an agent
//!   with `message = Some(note)` + the caller's
//!   `deliverer_agent_instance_hierarchy` (from
//!   `OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY`, verbatim) + no `psyop` /
//!   `score`.
//!
//! Every row also records `agent_kind` — whether the `agent` column is
//! an `agent_tag` or an `agent_instance_hierarchy`.
//!
//! Agent-side, the `read_queue` MCP tool lists pending entries and
//! `mark_handled` removes one.
//!
//! Storage: `<config_base_dir>/plugins/psychological-operations/queue.sqlite`,
//! a separate file from the response cache. WAL + 5 s busy timeout so
//! concurrent processes don't fail with `SQLITE_BUSY`. Schemas are
//! version-tracked in a `schema_version` table; bumping a constant
//! drops + recreates the affected table on next open.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use super::Error;
use super::locker;

// v4 adds `agent_kind` and renames `deliverer` ->
// `deliverer_agent_instance_hierarchy`. The queue carries only
// transient rows, so bumping the version just wipes + recreates the
// table on next open.
const QUEUE_VERSION: i64 = 4;

const QUEUE_CREATE: &str = "CREATE TABLE queue (\
        agent                              TEXT    NOT NULL,\
        agent_kind                         TEXT    NOT NULL,\
        tweet_id                           TEXT    NOT NULL,\
        psyop                              TEXT,\
        score                              REAL,\
        deliverer_agent_instance_hierarchy TEXT,\
        message                            TEXT,\
        queued_at                          INTEGER NOT NULL,\
        PRIMARY KEY (agent, tweet_id)\
    )";

/// How the `agent` column should be interpreted: a tag name, or an
/// agent-instance-hierarchy string. Stored as the snake_case TEXT
/// `"agent_tag"` / `"agent_instance_hierarchy"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    AgentTag,
    AgentInstanceHierarchy,
}

impl AgentKind {
    /// The TEXT form persisted in the `agent_kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::AgentTag => "agent_tag",
            AgentKind::AgentInstanceHierarchy => "agent_instance_hierarchy",
        }
    }

    /// Parse the `agent_kind` column back into the enum. Anything that
    /// isn't `"agent_tag"` is treated as an instance hierarchy.
    fn from_db(s: &str) -> AgentKind {
        match s {
            "agent_tag" => AgentKind::AgentTag,
            _ => AgentKind::AgentInstanceHierarchy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub agent: String,
    pub agent_kind: AgentKind,
    pub tweet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psyop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverer_agent_instance_hierarchy: Option<String>,
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
    /// Enables WAL + a 5 s busy timeout. Creates / upgrades the
    /// queue table on first open.
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
        .map_err(|e| Error::Other(format!("queue schema_version: {e}")))?;

        ensure_table(pool, "queue", QUEUE_VERSION, QUEUE_CREATE).await?;
        Ok(())
    }

    /// Upsert by `(agent, tweet_id)`. Re-enqueueing overwrites the
    /// other columns wholesale.
    pub async fn enqueue(&self, entry: &QueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT OR REPLACE INTO queue \
             (agent, agent_kind, tweet_id, psyop, score, \
              deliverer_agent_instance_hierarchy, message, queued_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.agent)
        .bind(entry.agent_kind.as_str())
        .bind(&entry.tweet_id)
        .bind(entry.psyop.as_deref())
        .bind(entry.score)
        .bind(entry.deliverer_agent_instance_hierarchy.as_deref())
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
            "SELECT agent, agent_kind, tweet_id, psyop, score, \
                    deliverer_agent_instance_hierarchy, message, queued_at \
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

    /// Distinct agents with at least one row. Operator-independent —
    /// the queue is shared. Returned in deterministic (alphabetical)
    /// order.
    pub async fn list_agents_with_entries(&self) -> Result<Vec<String>, Error> {
        let rows = sqlx::query(
            "SELECT DISTINCT agent FROM queue ORDER BY agent ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("queue list_agents: {e}")))?;
        Ok(rows.into_iter().map(|r| r.get::<String, _>("agent")).collect())
    }

    /// Delete `(agent, tweet_id)` if present. Returns `true` if a
    /// row was removed.
    pub async fn delete(
        &self,
        agent: &str,
        tweet_id: &str,
    ) -> Result<bool, Error> {
        let result = sqlx::query(
            "DELETE FROM queue WHERE agent = ? AND tweet_id = ?",
        )
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
        agent:      row.get("agent"),
        agent_kind: AgentKind::from_db(row.get::<String, _>("agent_kind").as_str()),
        tweet_id:   row.get("tweet_id"),
        psyop:      row.try_get("psyop").ok(),
        score:      row.try_get("score").ok(),
        deliverer_agent_instance_hierarchy: row
            .try_get("deliverer_agent_instance_hierarchy")
            .ok(),
        message:    row.try_get("message").ok(),
        queued_at:  row.get("queued_at"),
    }
}

/// Idempotent table create/upgrade. Reads the recorded version from
/// `schema_version` and, on missing-or-mismatched, drops the existing
/// table + runs `create_stmt` + stamps the new version. No-op when the
/// recorded version matches.
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
