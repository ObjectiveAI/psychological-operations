//! Per-agent Twitch (message) handling queue.
//!
//! Parallel to [`crate::discord_queue`] but for Twitch chat messages, keyed by
//! `(agent_tag, channel_login, message_id)`. Unused until Twitch psyops /
//! delivery exist; present now for symmetry with the X and Discord queues.

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwitchQueueEntry {
    pub agent_tag: String,
    pub channel_login: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psyop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverer_agent_instance_hierarchy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub queued_at: i64,
}

impl Db {
    /// Upsert by `(agent_tag, channel_login, message_id)`.
    pub async fn twitch_queue_enqueue(&self, entry: &TwitchQueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO twitch_queue \
             (agent_tag, channel_login, message_id, psyop, score, \
              deliverer_agent_instance_hierarchy, message, run_id, queued_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (agent_tag, channel_login, message_id) DO UPDATE SET \
              psyop = excluded.psyop, \
              score = excluded.score, \
              deliverer_agent_instance_hierarchy = excluded.deliverer_agent_instance_hierarchy, \
              message = excluded.message, \
              run_id = excluded.run_id, \
              queued_at = excluded.queued_at",
        )
        .bind(&entry.agent_tag)
        .bind(&entry.channel_login)
        .bind(&entry.message_id)
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
    pub async fn twitch_queue_list(&self, agent_tag: &str) -> Result<Vec<TwitchQueueEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, channel_login, message_id, psyop, score, \
                    deliverer_agent_instance_hierarchy, message, run_id, queued_at \
             FROM twitch_queue WHERE agent_tag = $1 ORDER BY queued_at ASC",
        )
        .bind(agent_tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Remove the given `(channel_login, message_id)` messages, all-or-nothing.
    /// Returns the keys that were NOT present (whole batch rolled back if any).
    pub async fn twitch_queue_delete_many(
        &self,
        agent_tag: &str,
        keys: &[(String, String)],
    ) -> Result<Vec<(String, String)>, Error> {
        let mut tx = self.pool.begin().await?;
        let mut missing = Vec::new();
        for (channel_login, message_id) in keys {
            let result = sqlx::query(
                "DELETE FROM twitch_queue \
                 WHERE agent_tag = $1 AND channel_login = $2 AND message_id = $3",
            )
            .bind(agent_tag)
            .bind(channel_login)
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                missing.push((channel_login.clone(), message_id.clone()));
            }
        }
        if missing.is_empty() {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(missing)
    }

    /// Count of pending messages for one `agent_tag`.
    pub async fn twitch_queue_count(&self, agent_tag: &str) -> Result<i64, Error> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM twitch_queue WHERE agent_tag = $1")
            .bind(agent_tag)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> TwitchQueueEntry {
    TwitchQueueEntry {
        agent_tag: row.get("agent_tag"),
        channel_login: row.get("channel_login"),
        message_id: row.get("message_id"),
        psyop: row.get::<Option<String>, _>("psyop"),
        score: row.get::<Option<f64>, _>("score"),
        deliverer_agent_instance_hierarchy: row
            .get::<Option<String>, _>("deliverer_agent_instance_hierarchy"),
        message: row.get::<Option<String>, _>("message"),
        run_id: row.get::<Option<String>, _>("run_id"),
        queued_at: row.get("queued_at"),
    }
}
