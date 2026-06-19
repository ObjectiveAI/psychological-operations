//! Deferred reply/quote queue.
//!
//! When X refuses an agent's `reply` or `quote` with the conversation-
//! restriction 403 (the account isn't allowed to engage that thread yet),
//! the x-api MCP server captures the attempt here instead of failing it,
//! and tells the agent it's been queued for later delivery (handled
//! separately).
//!
//! Rows are keyed by `(agent_tag, kind, target_tweet_id)` so there is at
//! most one pending reply AND one pending quote per agent per tweet.
//! Duplicate-reply/quote refusal lives elsewhere now — the x-api MCP's
//! per-target `actions` dedup (see `actions.rs`) blocks a second reply or
//! quote permanently, not just while one is queued here.
//!
//! `target_tweet_id` (the `in_reply_to_tweet_id` / `quote_tweet_id`) links
//! back to the tweet; `text` is the body. Together they are the complete
//! argument set for both tools.

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyQuoteEntry {
    pub agent_tag: String,
    /// `"reply"` or `"quote"`.
    pub kind: String,
    pub target_tweet_id: String,
    pub text: String,
    pub queued_at: i64,
}

impl Db {
    /// Upsert by `(agent_tag, kind, target_tweet_id)`. In practice the
    /// MCP per-target dedup means this is always an insert, but the
    /// upsert keeps it idempotent against races.
    pub async fn reply_quote_enqueue(&self, entry: &ReplyQuoteEntry) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO reply_quote_queue \
             (agent_tag, kind, target_tweet_id, text, queued_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (agent_tag, kind, target_tweet_id) DO UPDATE SET \
              text = excluded.text, \
              queued_at = excluded.queued_at",
        )
        .bind(&entry.agent_tag)
        .bind(&entry.kind)
        .bind(&entry.target_tweet_id)
        .bind(&entry.text)
        .bind(entry.queued_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every pending reply/quote, oldest first — the full batch the
    /// `agents deliver` driver hands to the browser.
    pub async fn reply_quote_list(&self) -> Result<Vec<ReplyQuoteEntry>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, kind, target_tweet_id, text, queued_at \
             FROM reply_quote_queue ORDER BY queued_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| ReplyQuoteEntry {
                agent_tag: row.get("agent_tag"),
                kind: row.get("kind"),
                target_tweet_id: row.get("target_tweet_id"),
                text: row.get("text"),
                queued_at: row.get("queued_at"),
            })
            .collect())
    }

    /// Remove one delivered entry by its primary key. Called as the
    /// browser confirms each delivery.
    pub async fn reply_quote_delete(
        &self,
        agent_tag: &str,
        kind: &str,
        target_tweet_id: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "DELETE FROM reply_quote_queue \
             WHERE agent_tag = $1 AND kind = $2 AND target_tweet_id = $3",
        )
        .bind(agent_tag)
        .bind(kind)
        .bind(target_tweet_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
