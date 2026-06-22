//! Per-psyop "delivered once" ledgers.
//!
//! One row records that a psyop has output an item for delivery. `psyops run`
//! filters its candidates against these tables (after de-dup) so a psyop never
//! re-delivers an item, and writes the survivors here as part of delivery. X is
//! keyed by `tweet_id` (`x_delivered`); Discord by `(channel_id, message_id)`
//! (`discord_delivered`).

use std::collections::HashSet;

use sqlx::Row;

use crate::{unix_now, Db, Error};

impl Db {
    /// Record that `psyop` delivered each of `tweet_ids` (idempotent). No-op
    /// on empty.
    pub async fn x_mark_delivered(&self, psyop: &str, tweet_ids: &[String]) -> Result<(), Error> {
        if tweet_ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO x_delivered (psyop, tweet_id, at)
             SELECT $1, t, $3 FROM UNNEST($2::text[]) AS t
             ON CONFLICT (psyop, tweet_id) DO NOTHING",
        )
        .bind(psyop)
        .bind(tweet_ids)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Of `tweet_ids`, the subset `psyop` has already delivered.
    pub async fn x_already_delivered(
        &self,
        psyop: &str,
        tweet_ids: &[String],
    ) -> Result<HashSet<String>, Error> {
        if tweet_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows =
            sqlx::query("SELECT tweet_id FROM x_delivered WHERE psyop = $1 AND tweet_id = ANY($2)")
                .bind(psyop)
                .bind(tweet_ids)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.get("tweet_id")).collect())
    }

    /// Record that `psyop` delivered each `(channel_id, message_id)`
    /// (idempotent). No-op on empty.
    pub async fn discord_mark_delivered(
        &self,
        psyop: &str,
        messages: &[(String, String)],
    ) -> Result<(), Error> {
        if messages.is_empty() {
            return Ok(());
        }
        let channel_ids: Vec<String> = messages.iter().map(|(c, _)| c.clone()).collect();
        let message_ids: Vec<String> = messages.iter().map(|(_, m)| m.clone()).collect();
        sqlx::query(
            "INSERT INTO discord_delivered (psyop, channel_id, message_id, at)
             SELECT $1, c, m, $4 FROM UNNEST($2::text[], $3::text[]) AS x(c, m)
             ON CONFLICT (psyop, channel_id, message_id) DO NOTHING",
        )
        .bind(psyop)
        .bind(&channel_ids)
        .bind(&message_ids)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Of `messages` (`(channel_id, message_id)`), the subset `psyop` has
    /// already delivered.
    pub async fn discord_already_delivered(
        &self,
        psyop: &str,
        messages: &[(String, String)],
    ) -> Result<HashSet<(String, String)>, Error> {
        if messages.is_empty() {
            return Ok(HashSet::new());
        }
        // Match on message_id (globally unique snowflake) scoped to this
        // psyop, then keep only the pairs we asked about.
        let message_ids: Vec<String> = messages.iter().map(|(_, m)| m.clone()).collect();
        let rows = sqlx::query(
            "SELECT channel_id, message_id FROM discord_delivered \
             WHERE psyop = $1 AND message_id = ANY($2)",
        )
        .bind(psyop)
        .bind(&message_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("channel_id"), r.get("message_id")))
            .collect())
    }
}
