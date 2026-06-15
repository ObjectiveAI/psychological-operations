//! Per-agent tweet handling queue (ported from the SDK's
//! `queue.sqlite`).
//!
//! Rows are keyed by `(agent, tweet_id)` — a shared "to-do list" per
//! X-API persona, **not** partitioned by operator. Two sources write:
//!
//! - **Psyop pipelines** — a tweet that scored above its threshold
//!   lands here with `psyop = Some(name)` + `score = Some(value)` + no
//!   `deliverer_agent_instance_hierarchy` / `message`.
//! - **`agents enqueue`** — an operator flags a tweet for an agent with
//!   `message = Some(note)` + the caller's
//!   `deliverer_agent_instance_hierarchy` + no `psyop` / `score`.
//!
//! Every row records `agent_kind` — whether `agent` is an `agent_tag`
//! or an `agent_instance_hierarchy`. Agent-side, the `read_queue` MCP
//! tool lists pending entries and `mark_handled` removes one or more
//! (atomically, all-or-nothing).

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

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

impl Db {
    /// Upsert by `(agent, tweet_id)`. Re-enqueueing overwrites the
    /// other columns wholesale.
    pub async fn queue_enqueue(&self, entry: &QueueEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO queue \
             (agent, agent_kind, tweet_id, psyop, score, \
              deliverer_agent_instance_hierarchy, message, queued_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (agent, tweet_id) DO UPDATE SET \
              agent_kind = excluded.agent_kind, \
              psyop = excluded.psyop, \
              score = excluded.score, \
              deliverer_agent_instance_hierarchy = excluded.deliverer_agent_instance_hierarchy, \
              message = excluded.message, \
              queued_at = excluded.queued_at",
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
        .await?;
        Ok(())
    }

    /// All entries for `agent`, oldest first.
    pub async fn queue_list(&self, agent: &str) -> Result<Vec<QueueEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent, agent_kind, tweet_id, psyop, score, \
                    deliverer_agent_instance_hierarchy, message, queued_at \
             FROM queue \
             WHERE agent = $1 \
             ORDER BY queued_at ASC",
        )
        .bind(agent)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_entry).collect())
    }

    /// Distinct agents with at least one row. Operator-independent —
    /// the queue is shared. Returned alphabetically.
    pub async fn queue_list_agents_with_entries(&self) -> Result<Vec<String>, Error> {
        let rows = sqlx::query("SELECT DISTINCT agent FROM queue ORDER BY agent ASC")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|r| r.get::<String, _>("agent")).collect())
    }

    /// Tweet counts grouped by `(agent, agent_kind)`. Single query;
    /// alphabetical by agent.
    pub async fn queue_counts_by_agent_kind(
        &self,
    ) -> Result<Vec<(String, AgentKind, i64)>, Error> {
        let rows = sqlx::query(
            "SELECT agent, agent_kind, COUNT(*) AS n \
             FROM queue \
             GROUP BY agent, agent_kind \
             ORDER BY agent ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>("agent"),
                    AgentKind::from_db(r.get::<String, _>("agent_kind").as_str()),
                    r.get::<i64, _>("n"),
                )
            })
            .collect())
    }

    /// Delete every `(agent, tweet_id)` in `tweet_ids` **atomically and
    /// all-or-nothing**: the deletes run in one transaction, and unless
    /// *every* requested id matched a row the transaction is rolled back —
    /// so the queue is left untouched whenever any id is absent.
    ///
    /// Returns the ids that were **not** present. An empty vec means all
    /// were deleted and the transaction committed; a non-empty vec means
    /// nothing was deleted (rolled back) and names the offending ids.
    pub async fn queue_delete_many(
        &self,
        agent: &str,
        tweet_ids: &[String],
    ) -> Result<Vec<String>, Error> {
        let mut tx = self.pool.begin().await?;
        let mut missing = Vec::new();
        for tweet_id in tweet_ids {
            let result = sqlx::query("DELETE FROM queue WHERE agent = $1 AND tweet_id = $2")
                .bind(agent)
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

fn row_to_entry(row: sqlx::postgres::PgRow) -> QueueEntry {
    QueueEntry {
        agent: row.get("agent"),
        agent_kind: AgentKind::from_db(row.get::<String, _>("agent_kind").as_str()),
        tweet_id: row.get("tweet_id"),
        psyop: row.get::<Option<String>, _>("psyop"),
        score: row.get::<Option<f64>, _>("score"),
        deliverer_agent_instance_hierarchy: row
            .get::<Option<String>, _>("deliverer_agent_instance_hierarchy"),
        message: row.get::<Option<String>, _>("message"),
        queued_at: row.get("queued_at"),
    }
}
