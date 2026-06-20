//! Per-psyop "delivered once" ledger.
//!
//! One row per `(psyop, tweet_id)` records that a psyop has output that
//! tweet for delivery. `psyops run` filters its candidates against this
//! table (after de-dup, before the `max_posts` cap) so a psyop never
//! re-delivers a tweet, and writes the survivors here as part of delivery.

use std::collections::HashSet;

use sqlx::Row;

use crate::{unix_now, Db, Error};

impl Db {
    /// Record that `psyop` delivered each of `tweet_ids` (idempotent; a
    /// `(psyop, tweet_id)` already present is left as-is). No-op on empty.
    pub async fn mark_delivered(&self, psyop: &str, tweet_ids: &[String]) -> Result<(), Error> {
        if tweet_ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO delivered (psyop, tweet_id, at)
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

    /// Of `tweet_ids`, the subset `psyop` has already delivered. Returns an
    /// empty set for an empty input (no query).
    pub async fn already_delivered(
        &self,
        psyop: &str,
        tweet_ids: &[String],
    ) -> Result<HashSet<String>, Error> {
        if tweet_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows =
            sqlx::query("SELECT tweet_id FROM delivered WHERE psyop = $1 AND tweet_id = ANY($2)")
                .bind(psyop)
                .bind(tweet_ids)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.get("tweet_id")).collect())
    }
}
