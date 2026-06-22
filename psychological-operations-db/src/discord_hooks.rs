//! Per-agent Discord daemon hooks.
//!
//! A hook is operator-defined Python, named per agent (PK `(agent_tag,
//! name)`). The daemon runs every hook for every gateway event it receives,
//! feeding the raw event JSON as the Python `input`. The daemon only opens a
//! listener for an agent that has BOTH discord auth and one or more hooks —
//! [`Db::discord_daemon_agents`] resolves exactly that set.

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordHookEntry {
    pub agent_tag: String,
    pub name: String,
    pub description: String,
    /// The hook's Python source.
    pub python: String,
    pub updated_at: i64,
}

impl Db {
    /// Upsert a hook by `(agent_tag, name)`. Re-adding the same name
    /// overwrites the description / python / timestamp.
    pub async fn discord_hook_set(&self, entry: &DiscordHookEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO discord_hooks \
             (agent_tag, name, description, python, updated_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (agent_tag, name) DO UPDATE SET \
              description = excluded.description, \
              python = excluded.python, \
              updated_at = excluded.updated_at",
        )
        .bind(&entry.agent_tag)
        .bind(&entry.name)
        .bind(&entry.description)
        .bind(&entry.python)
        .bind(entry.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All hooks for `agent_tag`, name order.
    pub async fn discord_hook_list(&self, agent_tag: &str) -> Result<Vec<DiscordHookEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, name, description, python, updated_at \
             FROM discord_hooks \
             WHERE agent_tag = $1 \
             ORDER BY name ASC",
        )
        .bind(agent_tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Delete one hook by `(agent_tag, name)`. Returns `true` if a row was
    /// removed, `false` if none matched.
    pub async fn discord_hook_delete(&self, agent_tag: &str, name: &str) -> Result<bool, Error> {
        let result = sqlx::query("DELETE FROM discord_hooks WHERE agent_tag = $1 AND name = $2")
            .bind(agent_tag)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// The agent tags the daemon should listen for: those with BOTH discord
    /// auth (a non-null bot token) AND at least one hook.
    pub async fn discord_daemon_agents(&self) -> Result<Vec<String>, Error> {
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT h.agent_tag FROM discord_hooks h \
             JOIN discord_auth a ON a.agent_tag = h.agent_tag \
             WHERE a.bot_token IS NOT NULL \
             ORDER BY h.agent_tag ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(tags)
    }
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> DiscordHookEntry {
    DiscordHookEntry {
        agent_tag: row.get("agent_tag"),
        name: row.get("name"),
        description: row.get("description"),
        python: row.get("python"),
        updated_at: row.get("updated_at"),
    }
}
