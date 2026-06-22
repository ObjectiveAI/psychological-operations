//! Per-agent X (tweet) handling queue.
//!
//! Rows are keyed by `(agent_tag, tweet_id)` — a shared "to-do list" per
//! agent, **not** partitioned by operator. Two sources write:
//!
//! - **Psyop pipelines** — a survivor lands here with `psyop = Some(name)`
//!   + `score = Some(value)` + `run_id = Some(id)` (shared by every row a
//!   single psyop run enqueues, so readers can group them) + the running
//!   agent's `deliverer_agent_instance_hierarchy` + no `message`.
//! - **`agents enqueue x`** — an operator flags a tweet for an agent with
//!   `message = Some(note)` + the caller's
//!   `deliverer_agent_instance_hierarchy` + no `psyop` / `score` / `run_id`.
//!
//! The `agent_tag` is the agent's tag (the only agent identity). Agent-
//! side, the `read_queue` MCP tool lists pending entries and
//! `mark_handled` removes one or more (atomically, all-or-nothing).

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XQueueEntry {
    pub agent_tag: String,
    pub tweet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psyop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverer_agent_instance_hierarchy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Shared by every row a single psyop run enqueues (so the MCP can
    /// group them into one item); `None` for operator-enqueued rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub queued_at: i64,
}

impl Db {
    /// Upsert by `(agent_tag, tweet_id)`. Re-enqueueing overwrites the
    /// other columns wholesale.
    pub async fn x_queue_enqueue(&self, entry: &XQueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO x_queue \
             (agent_tag, tweet_id, psyop, score, \
              deliverer_agent_instance_hierarchy, message, run_id, queued_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (agent_tag, tweet_id) DO UPDATE SET \
              psyop = excluded.psyop, \
              score = excluded.score, \
              deliverer_agent_instance_hierarchy = excluded.deliverer_agent_instance_hierarchy, \
              message = excluded.message, \
              run_id = excluded.run_id, \
              queued_at = excluded.queued_at",
        )
        .bind(&entry.agent_tag)
        .bind(&entry.tweet_id)
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
    pub async fn x_queue_list(&self, agent_tag: &str) -> Result<Vec<XQueueEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, tweet_id, psyop, score, \
                    deliverer_agent_instance_hierarchy, message, run_id, queued_at \
             FROM x_queue \
             WHERE agent_tag = $1 \
             ORDER BY queued_at ASC",
        )
        .bind(agent_tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Count of pending tweets for one `agent_tag` — the number reported
    /// in the agent's notification.
    pub async fn x_queue_count(&self, agent_tag: &str) -> Result<i64, Error> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM x_queue WHERE agent_tag = $1")
            .bind(agent_tag)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    /// Delete every `(agent_tag, tweet_id)` in `tweet_ids` **atomically and
    /// all-or-nothing**: the deletes run in one transaction, and unless
    /// *every* requested id matched a row the transaction is rolled back —
    /// so the queue is left untouched whenever any id is absent.
    ///
    /// Returns the ids that were **not** present. An empty vec means all
    /// were deleted and the transaction committed; a non-empty vec means
    /// nothing was deleted (rolled back) and names the offending ids.
    pub async fn x_queue_delete_many(
        &self,
        agent_tag: &str,
        tweet_ids: &[String],
    ) -> Result<Vec<String>, Error> {
        let mut tx = self.pool.begin().await?;
        let mut missing = Vec::new();
        for tweet_id in tweet_ids {
            let result = sqlx::query("DELETE FROM x_queue WHERE agent_tag = $1 AND tweet_id = $2")
                .bind(agent_tag)
                .bind(tweet_id)
                .execute(&mut *tx)
                .await?;
            if result.rows_affected() == 0 {
                missing.push(tweet_id.clone());
            }
        }
        if missing.is_empty() {
            tx.commit().await?;
        } else {
            // All-or-nothing: a single absent id voids the whole batch.
            tx.rollback().await?;
        }
        Ok(missing)
    }
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> XQueueEntry {
    XQueueEntry {
        agent_tag: row.get("agent_tag"),
        tweet_id: row.get("tweet_id"),
        psyop: row.get::<Option<String>, _>("psyop"),
        score: row.get::<Option<f64>, _>("score"),
        deliverer_agent_instance_hierarchy: row
            .get::<Option<String>, _>("deliverer_agent_instance_hierarchy"),
        message: row.get::<Option<String>, _>("message"),
        run_id: row.get::<Option<String>, _>("run_id"),
        queued_at: row.get("queued_at"),
    }
}
