//! Psyop pipeline DTOs + the run-interval stamp.
//!
//! The candidate pipeline (scrape → query → hydrate → filter → sort →
//! score) is **in-memory only** — it lives for the duration of a single
//! `psyops run` invocation and never touches the database. So this module
//! no longer owns `posts`/`sources`/`contents`/`scores`/`for_you_queue`;
//! it keeps just the plain DTOs the in-memory pipeline passes around
//! ([`Post`], [`MediaUrl`], [`Origin`]) and the one thing that IS durable:
//! the `psyop_runs` interval stamp ([`Db::get_last_run`] /
//! [`Db::set_last_run`]). Survivors are delivered via the agent `queue`
//! (see `queue.rs`), which is separate.

use serde::{Deserialize, Serialize};

use crate::{Db, Error};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaUrl {
    pub url: String,
}

/// Canonical tweet content + engagement metadata — the in-memory shape
/// scraped/queried/hydrated tweets take through the run pipeline.
/// `Serialize`/`Deserialize` so the stage-retry ledger can persist a
/// failed run's stage input as JSONB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub handle: String,
    pub text: String,
    pub images: Vec<MediaUrl>,
    pub videos: Vec<MediaUrl>,
    pub created: String,
    pub likes: u64,
    pub retweets: u64,
    pub replies: u64,
    pub impressions: u64,
}

/// Which input on a psyop produced a candidate.
#[derive(Debug, Clone)]
pub enum Origin {
    /// Collected from an agent's For You feed; carries the collecting
    /// agent's tag (used to resolve the psyop's per-agent `ForYou`
    /// filter/priority and to interweave by agent at sort time).
    ForYou(String),
    Query(String),
}

impl Db {
    /// Unix seconds of this psyop's last successful run, or `None`.
    pub async fn get_last_run(&self, psyop: &str) -> Result<Option<i64>, Error> {
        let out: Option<i64> =
            sqlx::query_scalar("SELECT last_run_at FROM psyop_runs WHERE psyop = $1")
                .bind(psyop)
                .fetch_optional(&self.pool)
                .await?;
        Ok(out)
    }

    /// Record `at` (unix seconds) as this psyop's last successful run.
    pub async fn set_last_run(&self, psyop: &str, at: i64) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO psyop_runs (psyop, last_run_at)
             VALUES ($1, $2)
             ON CONFLICT (psyop) DO UPDATE SET last_run_at = excluded.last_run_at",
        )
        .bind(psyop)
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
