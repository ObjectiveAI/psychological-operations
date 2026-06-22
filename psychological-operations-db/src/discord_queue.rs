//! Per-agent Discord (message) handling queue.
//!
//! Parallel to [`crate::x_queue`] but for Discord messages. Rows are keyed by
//! `(agent_tag, channel_id, message_id)` — a Discord message needs both ids
//! to be addressable (`GET /channels/{channel_id}/messages/{message_id}`);
//! `guild_id` is optional context (absent for DMs). Two sources write:
//!
//! - **Psyop pipelines** — a survivor lands here with `psyop` + `score` +
//!   `run_id` + the running agent's `deliverer_agent_instance_hierarchy`.
//! - **`agents enqueue discord`** — an operator flags a message for an agent
//!   with `message = Some(note)` + the caller's
//!   `deliverer_agent_instance_hierarchy`.

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordQueueEntry {
    pub agent_tag: String,
    pub channel_id: String,
    pub message_id: String,
    /// Server the message is in; `None` for DMs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psyop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverer_agent_instance_hierarchy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Shared by every row a single psyop run enqueues (so readers can group
    /// them); `None` for operator-enqueued rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub queued_at: i64,
}

impl Db {
    /// Upsert by `(agent_tag, channel_id, message_id)`. Re-enqueueing
    /// overwrites the other columns wholesale.
    pub async fn discord_queue_enqueue(&self, entry: &DiscordQueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO discord_queue \
             (agent_tag, channel_id, message_id, guild_id, psyop, score, \
              deliverer_agent_instance_hierarchy, message, run_id, queued_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (agent_tag, channel_id, message_id) DO UPDATE SET \
              guild_id = excluded.guild_id, \
              psyop = excluded.psyop, \
              score = excluded.score, \
              deliverer_agent_instance_hierarchy = excluded.deliverer_agent_instance_hierarchy, \
              message = excluded.message, \
              run_id = excluded.run_id, \
              queued_at = excluded.queued_at",
        )
        .bind(&entry.agent_tag)
        .bind(&entry.channel_id)
        .bind(&entry.message_id)
        .bind(entry.guild_id.as_deref())
        .bind(entry.psyop.as_deref())
        .bind(entry.score)
        .bind(entry.deliverer_agent_instance_hierarchy.as_deref())
        .bind(entry.message.as_deref())
        .bind(entry.run_id.as_deref())
        .bind(entry.queued_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All entries for `agent_tag`, oldest first.
    pub async fn discord_queue_list(
        &self,
        agent_tag: &str,
    ) -> Result<Vec<DiscordQueueEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, channel_id, message_id, guild_id, psyop, score, \
                    deliverer_agent_instance_hierarchy, message, run_id, queued_at \
             FROM discord_queue \
             WHERE agent_tag = $1 \
             ORDER BY queued_at ASC",
        )
        .bind(agent_tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Count of pending messages for one `agent_tag`.
    pub async fn discord_queue_count(&self, agent_tag: &str) -> Result<i64, Error> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM discord_queue WHERE agent_tag = $1")
            .bind(agent_tag)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> DiscordQueueEntry {
    DiscordQueueEntry {
        agent_tag: row.get("agent_tag"),
        channel_id: row.get("channel_id"),
        message_id: row.get("message_id"),
        guild_id: row.get::<Option<String>, _>("guild_id"),
        psyop: row.get::<Option<String>, _>("psyop"),
        score: row.get::<Option<f64>, _>("score"),
        deliverer_agent_instance_hierarchy: row
            .get::<Option<String>, _>("deliverer_agent_instance_hierarchy"),
        message: row.get::<Option<String>, _>("message"),
        run_id: row.get::<Option<String>, _>("run_id"),
        queued_at: row.get("queued_at"),
    }
}
