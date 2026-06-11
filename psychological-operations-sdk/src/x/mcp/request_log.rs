//! Per-caller X-API request log + sliding-window quota ledger.
//!
//! Every real X-API HTTP request the MCP's client fires is logged
//! here: the method, the full URL, and the caller's
//! `agent_instance_hierarchy` (from the MCP session) — never the
//! request arguments. Rows are permanent; this is an audit log
//! first, and the quota mechanism falls out of it: a caller's
//! quota check is simply "how many same-class (read = GET,
//! write = everything else) rows has this hierarchy logged in the
//! trailing hour" — no buckets, no schedules, no resets.
//!
//! [`RequestLogStore::try_log`] is the whole quota protocol: one
//! guarded `INSERT … SELECT … WHERE count < limit` statement, so
//! the check and the deduction (the inserted row) are atomic under
//! the MCP server's concurrent tool calls.
//!
//! Storage: `<config_base_dir>/plugins-state/psychological-operations/x-api-mcp.sqlite`,
//! shared with the engagement store (same `schema_version`
//! pattern, sibling of `x-api-cache.sqlite` and `queue.sqlite`).
//! WAL + 5 s busy timeout.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use super::super::Error;
use super::super::locker;

const API_REQUESTS_VERSION: i64 = 1;

const API_REQUESTS_CREATE: &str = "CREATE TABLE api_requests (\
        id                       INTEGER PRIMARY KEY AUTOINCREMENT,\
        agent_instance_hierarchy TEXT    NOT NULL,\
        method                   TEXT    NOT NULL,\
        url                      TEXT    NOT NULL,\
        requested_at             INTEGER NOT NULL\
    )";

const API_REQUESTS_INDEX: &str = "CREATE INDEX IF NOT EXISTS \
    api_requests_caller_time ON api_requests \
    (agent_instance_hierarchy, requested_at)";

/// SQLite-backed API request log. Open via
/// [`RequestLogStore::open`] or — preferred — through
/// `Client::request_log()` (lazy `OnceCell`).
pub struct RequestLogStore {
    pool: SqlitePool,
}

impl std::fmt::Debug for RequestLogStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestLogStore").finish_non_exhaustive()
    }
}

impl RequestLogStore {
    /// Open (creating if missing) the request-log table inside
    /// `<config_base_dir>/plugins-state/psychological-operations/x-api-mcp.sqlite`.
    /// Enables WAL + a 5 s busy timeout.
    pub async fn open(config_base_dir: &Path) -> Result<Self, Error> {
        let pool = open_pool(config_base_dir).await?;
        Self::ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        // Idempotent; the engagement store shares this file + table.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_version (\
                 name    TEXT    PRIMARY KEY,\
                 version INTEGER NOT NULL\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("request log schema_version: {e}")))?;

        ensure_table(pool, "api_requests", API_REQUESTS_VERSION, API_REQUESTS_CREATE).await?;
        // IF NOT EXISTS — the index dies with the table on a
        // version-bump drop, so re-running unconditionally is right.
        sqlx::query(API_REQUESTS_INDEX)
            .execute(pool)
            .await
            .map_err(|e| Error::Other(format!("api_requests index: {e}")))?;
        Ok(())
    }

    /// Atomic check-and-deduct. Classifies `method` as read (GET)
    /// or write (anything else), counts the caller's same-class
    /// rows in the trailing hour, and inserts the new log row iff
    /// that count is below `limit` — one guarded statement, so
    /// concurrent callers can't both squeeze through the last slot.
    ///
    /// `Ok(true)` — logged; the request may fire.
    /// `Ok(false)` — quota hit; nothing was logged.
    pub async fn try_log(
        &self,
        agent_instance_hierarchy: &str,
        method: &str,
        url: &str,
        limit: u64,
    ) -> Result<bool, Error> {
        let now = locker::unix_now();
        let cutoff = now - 3600;
        let inserted = sqlx::query(
            "INSERT INTO api_requests \
                 (agent_instance_hierarchy, method, url, requested_at) \
             SELECT ?1, ?2, ?3, ?4 \
             WHERE (SELECT COUNT(*) FROM api_requests \
                    WHERE agent_instance_hierarchy = ?1 \
                      AND requested_at > ?5 \
                      AND (method = 'GET') = (?2 = 'GET')) < ?6",
        )
        .bind(agent_instance_hierarchy)
        .bind(method)
        .bind(url)
        .bind(now)
        .bind(cutoff)
        .bind(limit as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("request log insert: {e}")))?
        .rows_affected();
        Ok(inserted > 0)
    }

    /// `(reads, writes)` this caller has logged in the trailing
    /// hour — the same counts [`Self::try_log`] gates on. Read
    /// AFTER a tool's API calls, it reflects that call's own
    /// deductions.
    pub async fn usage(
        &self,
        agent_instance_hierarchy: &str,
    ) -> Result<(u64, u64), Error> {
        let cutoff = locker::unix_now() - 3600;
        let row: (i64, i64) = sqlx::query_as(
            "SELECT \
                 COALESCE(SUM(method = 'GET'), 0), \
                 COALESCE(SUM(method != 'GET'), 0) \
             FROM api_requests \
             WHERE agent_instance_hierarchy = ? AND requested_at > ?",
        )
        .bind(agent_instance_hierarchy)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("request log usage: {e}")))?;
        Ok((row.0.max(0) as u64, row.1.max(0) as u64))
    }
}

/// Idempotent table create/upgrade. Mirrors
/// `engagement.rs::ensure_table` (which mirrors queue.rs's).
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

/// Open the request log's SQLite file — the same `x-api-mcp.sqlite`
/// the engagement store uses, under
/// `<config>/plugins-state/psychological-operations/`.
async fn open_pool(config_base_dir: &Path) -> Result<SqlitePool, Error> {
    let dir = config_base_dir
        .join("plugins-state")
        .join("psychological-operations");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(format!("request log mkdir: {e}")))?;
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
        .map_err(|e| Error::Other(format!("request log pool open {}: {e}", path.display())))
}
