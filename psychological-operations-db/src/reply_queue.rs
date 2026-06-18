//! Deferred reply/quote queue.
//!
//! When X refuses an agent's `reply` or `quote` with the conversation-
//! restriction 403 (the account isn't allowed to engage that thread yet),
//! the x-api MCP server captures the attempt here instead of failing it,
//! and tells the agent it's been queued for later delivery (handled
//! separately).
//!
//! Rows are keyed by `(agent_tag, kind, target_tweet_id)` so there is at
//! most one pending reply AND one pending quote per agent per tweet. That
//! key is also what the MCP server's pending pre-check reads: a new reply
//! is refused only while a reply is pending for the same tweet, a quote
//! only while a quote is pending — never cross-blocking.
//!
//! `target_tweet_id` (the `in_reply_to_tweet_id` / `quote_tweet_id`) links
//! back to the tweet; `text` is the body. Together they are the complete
//! argument set for both tools.

use serde::{Deserialize, Serialize};

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
    /// MCP pending pre-check means this is always an insert, but the
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

    /// Whether a `kind` (`"reply"` / `"quote"`) is already pending for this
    /// `(agent_tag, target_tweet_id)` — the MCP server's duplicate guard.
    pub async fn reply_quote_pending_exists(
        &self,
        agent_tag: &str,
        kind: &str,
        target_tweet_id: &str,
    ) -> Result<bool, Error> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM reply_quote_queue \
             WHERE agent_tag = $1 AND kind = $2 AND target_tweet_id = $3)",
        )
        .bind(agent_tag)
        .bind(kind)
        .bind(target_tweet_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }
}
