//! Per-(agent, target) engagement record (ported from the SDK's
//! `x-api-mcp.sqlite`).
//!
//! Tweet-scoped engagements (`replies`, `retweets`, `likes`, `quotes`)
//! live in four parallel tables, each keyed by `(tweet_id, agent)`. A
//! fifth table `follows` records user-scoped engagements keyed by
//! `(user_id, agent)`. All five are additive: a row's presence means
//! "this agent performed this engagement at some point."
//!
//! [`Db::engagement_get`] joins the four tweet tables in a single query
//! and returns the bool-struct. [`Db::engagement_is_following`] is a
//! separate single-row probe. Each `mark_*` is an idempotent insert.

use serde::{Deserialize, Serialize};

use crate::{unix_now, Db, Error};

/// Whether `(agent, tweet_id)` has been recorded in each of the four
/// tweet engagement tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Engagement {
    pub replied: bool,
    pub retweeted: bool,
    pub liked: bool,
    pub quoted: bool,
}

impl Db {
    /// Combined existence check across all four tweet tables in a
    /// single round-trip.
    pub async fn engagement_get(&self, agent: &str, tweet_id: &str) -> Result<Engagement, Error> {
        let row: (bool, bool, bool, bool) = sqlx::query_as(
            "SELECT \
                EXISTS(SELECT 1 FROM replies   WHERE tweet_id = $1 AND agent = $2) AS replied, \
                EXISTS(SELECT 1 FROM retweets  WHERE tweet_id = $1 AND agent = $2) AS retweeted, \
                EXISTS(SELECT 1 FROM likes     WHERE tweet_id = $1 AND agent = $2) AS liked, \
                EXISTS(SELECT 1 FROM quotes    WHERE tweet_id = $1 AND agent = $2) AS quoted",
        )
        .bind(tweet_id)
        .bind(agent)
        .fetch_one(&self.pool)
        .await?;
        Ok(Engagement {
            replied: row.0,
            retweeted: row.1,
            liked: row.2,
            quoted: row.3,
        })
    }

    /// Record `(tweet_id, agent)` in `replies`.
    pub async fn engagement_mark_replied(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.engagement_insert("replies", "tweet_id", agent, tweet_id)
            .await
    }

    /// Record `(tweet_id, agent)` in `retweets`.
    pub async fn engagement_mark_retweeted(
        &self,
        agent: &str,
        tweet_id: &str,
    ) -> Result<(), Error> {
        self.engagement_insert("retweets", "tweet_id", agent, tweet_id)
            .await
    }

    /// Record `(tweet_id, agent)` in `likes`.
    pub async fn engagement_mark_liked(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.engagement_insert("likes", "tweet_id", agent, tweet_id)
            .await
    }

    /// Record `(tweet_id, agent)` in `quotes`.
    pub async fn engagement_mark_quoted(&self, agent: &str, tweet_id: &str) -> Result<(), Error> {
        self.engagement_insert("quotes", "tweet_id", agent, tweet_id)
            .await
    }

    /// Whether `(user_id, agent)` is recorded in the `follows` table.
    /// Single-row probe — follows aren't bundled into the per-tweet
    /// [`Engagement`] struct because they're keyed by X user id.
    pub async fn engagement_is_following(&self, agent: &str, user_id: &str) -> Result<bool, Error> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM follows WHERE user_id = $1 AND agent = $2) AS following",
        )
        .bind(user_id)
        .bind(agent)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    /// Record `(user_id, agent)` in `follows`.
    pub async fn engagement_mark_followed(&self, agent: &str, user_id: &str) -> Result<(), Error> {
        self.engagement_insert("follows", "user_id", agent, user_id)
            .await
    }

    /// Shared idempotent insert. `table`/`key_col` are fixed string
    /// literals from the `mark_*` wrappers — never user input.
    async fn engagement_insert(
        &self,
        table: &str,
        key_col: &str,
        agent: &str,
        key_value: &str,
    ) -> Result<(), Error> {
        let sql = format!(
            "INSERT INTO {table} ({key_col}, agent, created_at) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING"
        );
        sqlx::query(&sql)
            .bind(key_value)
            .bind(agent)
            .bind(unix_now())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
