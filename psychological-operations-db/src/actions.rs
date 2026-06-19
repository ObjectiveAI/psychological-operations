//! Per-agent, per-target action idempotency.
//!
//! One row per `(account, action, target)` records that an agent has
//! already taken an engagement against a target, so the x-api MCP can
//! refuse a repeat: at most one `like`/`retweet`/`quote`/`reply` per tweet
//! and one `follow` per handle.
//!
//! `target` is the tweet ID for like/retweet/quote/reply, and the
//! normalized handle for follow. `quote` and `retweet` are mutually
//! exclusive for a tweet — the MCP pre-check passes BOTH names to
//! [`Db::action_taken`] so either blocks the other. `unfollow` calls
//! [`Db::remove_action`] to delete the `follow` row, re-allowing a later
//! follow of that handle.

use crate::{unix_now, Db, Error};

impl Db {
    /// Whether `account` has already taken any of `actions` against
    /// `target`. Pass multiple action names to express mutual exclusion
    /// (e.g. `["quote", "retweet"]`).
    pub async fn action_taken(
        &self,
        account: &str,
        actions: &[&str],
        target: &str,
    ) -> Result<bool, Error> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM actions \
             WHERE account = $1 AND action = ANY($2) AND target = $3)",
        )
        .bind(account)
        .bind(actions)
        .bind(target)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Record that `account` took `action` against `target`. Idempotent —
    /// a duplicate `(account, action, target)` is a no-op.
    pub async fn record_action(
        &self,
        account: &str,
        action: &str,
        target: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO actions (account, action, target, at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (account, action, target) DO NOTHING",
        )
        .bind(account)
        .bind(action)
        .bind(target)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove the `(account, action, target)` record, re-allowing that
    /// action. Used by `unfollow` to clear the `follow` row.
    pub async fn remove_action(
        &self,
        account: &str,
        action: &str,
        target: &str,
    ) -> Result<(), Error> {
        sqlx::query("DELETE FROM actions WHERE account = $1 AND action = $2 AND target = $3")
            .bind(account)
            .bind(action)
            .bind(target)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
