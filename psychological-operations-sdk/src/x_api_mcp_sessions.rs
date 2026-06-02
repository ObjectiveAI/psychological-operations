//! Per-X-API-MCP-session persistence.
//!
//! Single sqlite file at
//! `<config_base_dir>/plugins/psychological-operations/x-api-mcp-sessions.sqlite`,
//! WAL mode + 5 s busy timeout — same connection pattern as
//! [`crate::x::cache`], independent file so heavy session
//! initialize / close churn doesn't slow X-API response-cache
//! reads.
//!
//! No per-key locking: sessions are unique by construction (each
//! row keyed by an rmcp-minted session id), so there is no
//! get-or-fetch stampede to coalesce.
//!
//! The `mode` column is a free-form string — the SDK doesn't
//! know about MCP tool surfaces. The MCP layer maps its
//! `Mode::Readonly` / `Mode::Full` enum to `"readonly"` /
//! `"full"` on the way in and parses on the way out.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use crate::x::Error;

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub agent: String,
    pub mode: String,
    /// Unix epoch seconds at which this session was first
    /// recorded.
    pub created_at: i64,
}

/// SQLite-backed `SessionId → SessionRecord` store.
#[derive(Debug, Clone)]
pub struct SessionStore {
    pool: SqlitePool,
}

impl SessionStore {
    /// Open (creating if missing) the sessions sqlite file under
    /// `<config_base_dir>/plugins/psychological-operations/x-api-mcp-sessions.sqlite`.
    /// Enables WAL + 5 s busy timeout so concurrent
    /// initialize / close traffic doesn't fail with `SQLITE_BUSY`.
    pub async fn open(config_base_dir: &Path) -> Result<Self, Error> {
        let pool = open_pool(config_base_dir).await?;
        Self::ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    async fn ensure_schema(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (\
                 session_id TEXT PRIMARY KEY NOT NULL,\
                 agent      TEXT NOT NULL,\
                 mode       TEXT NOT NULL,\
                 created_at INTEGER NOT NULL\
             )",
        )
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("x_api_mcp_sessions schema: {e}")))?;
        Ok(())
    }

    /// All persisted sessions, in insertion order (oldest first).
    /// Callers use this on server startup to rehydrate in-memory
    /// state.
    pub async fn list(&self) -> Result<Vec<SessionRecord>, Error> {
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT session_id, agent, mode, created_at \
             FROM sessions ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("x_api_mcp_sessions list: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|(session_id, agent, mode, created_at)| SessionRecord {
                session_id,
                agent,
                mode,
                created_at,
            })
            .collect())
    }

    /// Persist (or overwrite) a session row.
    pub async fn insert(&self, rec: &SessionRecord) -> Result<(), Error> {
        sqlx::query(
            "INSERT OR REPLACE INTO sessions \
                 (session_id, agent, mode, created_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&rec.session_id)
        .bind(&rec.agent)
        .bind(&rec.mode)
        .bind(rec.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Other(format!("x_api_mcp_sessions insert: {e}")))?;
        Ok(())
    }

    /// Delete a session row. No-op if the row doesn't exist.
    pub async fn remove(&self, session_id: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Other(format!("x_api_mcp_sessions remove: {e}")))?;
        Ok(())
    }
}

async fn open_pool(config_base_dir: &Path) -> Result<SqlitePool, Error> {
    let dir = config_base_dir
        .join("plugins")
        .join("psychological-operations");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(format!("x_api_mcp_sessions mkdir: {e}")))?;
    let path = dir.join("x-api-mcp-sessions.sqlite");

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(100)
        .connect_with(opts)
        .await
        .map_err(|e| {
            Error::Other(format!("x_api_mcp_sessions pool open {}: {e}", path.display()))
        })
}
