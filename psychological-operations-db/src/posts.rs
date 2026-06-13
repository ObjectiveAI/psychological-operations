//! The psyop pipeline tables (ported from the CLI's old `data.db`):
//! posts, sources, contents, scores, for_you_queue, delivery_queue,
//! psyop_runs. Keyed by psyop **name** only — `psyop_commit_sha` is
//! gone (git versioning was dropped).

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Db, Error};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaUrl {
    pub url: String,
}

/// Canonical tweet content + engagement metadata (insert input).
#[derive(Debug, Clone)]
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

/// Which input on a psyop produced this post. Mirrors the
/// `(for_you, query)` column pair on the `sources` table.
#[derive(Debug, Clone)]
pub enum Origin {
    ForYou,
    Query(String),
}

/// One unscored post row joined with its origins, for the
/// filter/sort/score read path. `seq` is the monotonic insertion
/// order (for_you arrival order). The CLI maps this to its `Tweet`
/// (computing `age` from `created`).
#[derive(Debug, Clone)]
pub struct PostRow {
    pub id: String,
    pub handle: String,
    pub created: String,
    pub likes: u64,
    pub retweets: u64,
    pub replies: u64,
    pub impressions: u64,
    pub seq: i64,
}

/// One row from `delivery_queue` — a delivery awaiting (re)delivery.
#[derive(Debug, Clone)]
pub struct QueuedDelivery {
    pub id: i64,
    pub psyop: String,
    pub target: serde_json::Value,
    pub post_ids: serde_json::Value,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<String>,
}

impl Db {
    /// Ingest a post under `psyop` with the given origin.
    ///
    /// No-op (returns `Ok(false)`) if the post is already scored.
    /// Otherwise, in one transaction: insert-or-ignore the posts row
    /// (first observation wins), insert-or-ignore the source row, and
    /// insert-or-ignore the contents row. Returns `true` iff a *new
    /// source* row was created.
    pub async fn insert_post(
        &self,
        post: &Post,
        psyop: &str,
        origin: &Origin,
    ) -> Result<bool, Error> {
        let (for_you, query) = match origin {
            Origin::ForYou => (true, None),
            Origin::Query(q) => (false, Some(q.as_str())),
        };
        let mut tx = self.pool.begin().await?;

        let already_scored: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM scores WHERE post_id = $1 LIMIT 1")
                .bind(&post.id)
                .fetch_optional(&mut *tx)
                .await?;
        if already_scored.is_some() {
            tx.commit().await?;
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO posts
                (id, psyop, handle, created, likes, retweets, replies, impressions)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id, psyop) DO NOTHING",
        )
        .bind(&post.id)
        .bind(psyop)
        .bind(&post.handle)
        .bind(&post.created)
        .bind(post.likes as i64)
        .bind(post.retweets as i64)
        .bind(post.replies as i64)
        .bind(post.impressions as i64)
        .execute(&mut *tx)
        .await?;

        let source_inserted = sqlx::query(
            "INSERT INTO sources (post_id, for_you, query)
             VALUES ($1, $2, $3)
             ON CONFLICT (post_id, COALESCE(query, '')) DO NOTHING",
        )
        .bind(&post.id)
        .bind(for_you)
        .bind(query)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        let images = serde_json::to_value(&post.images)?;
        let videos = serde_json::to_value(&post.videos)?;
        sqlx::query(
            "INSERT INTO contents (post_id, text, images, videos)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (post_id) DO NOTHING",
        )
        .bind(&post.id)
        .bind(&post.text)
        .bind(&images)
        .bind(&videos)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(source_inserted)
    }

    /// Browser-extension entry point: queue a `(post_id, psyop)` pair
    /// for later runtime hydration. Returns `true` iff a new row was
    /// created.
    pub async fn enqueue_for_you(&self, post_id: &str, psyop: &str) -> Result<bool, Error> {
        let n = sqlx::query(
            "INSERT INTO for_you_queue (post_id, psyop)
             VALUES ($1, $2)
             ON CONFLICT (post_id, psyop) DO NOTHING",
        )
        .bind(post_id)
        .bind(psyop)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n > 0)
    }

    /// Every queued post_id for `psyop`, oldest first.
    pub async fn for_you_queue(&self, psyop: &str) -> Result<Vec<String>, Error> {
        let out: Vec<String> = sqlx::query_scalar(
            "SELECT post_id FROM for_you_queue
              WHERE psyop = $1
              ORDER BY ingested_at ASC",
        )
        .bind(psyop)
        .fetch_all(&self.pool)
        .await?;
        Ok(out)
    }

    /// Drop queue rows after successful hydration. Caller passes only
    /// the ids it actually persisted.
    pub async fn dequeue_for_you(&self, psyop: &str, post_ids: &[String]) -> Result<(), Error> {
        if post_ids.is_empty() {
            return Ok(());
        }
        sqlx::query("DELETE FROM for_you_queue WHERE psyop = $1 AND post_id = ANY($2)")
            .bind(psyop)
            .bind(post_ids)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every unscored posts row for `psyop`, paired with all of its
    /// origins. Rows arrive collapsed one-per-post. The CLI maps each
    /// `PostRow` to its `Tweet` (computing age).
    pub async fn list_unscored_with_origins(
        &self,
        psyop: &str,
    ) -> Result<Vec<(PostRow, Vec<Origin>)>, Error> {
        let rows = sqlx::query(
            "SELECT
                p.id, p.handle, p.created,
                p.likes, p.retweets, p.replies, p.impressions,
                s.for_you, s.query,
                p.seq AS seq
             FROM posts p
             LEFT JOIN sources s ON s.post_id = p.id
             WHERE p.psyop = $1
               AND NOT EXISTS (SELECT 1 FROM scores sc WHERE sc.post_id = p.id)
             ORDER BY p.id",
        )
        .bind(psyop)
        .fetch_all(&self.pool)
        .await?;

        let mut out: Vec<(PostRow, Vec<Origin>)> = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            let for_you: Option<bool> = row.get("for_you");
            let query: Option<String> = row.get("query");

            let origin = match for_you {
                Some(true) => Some(Origin::ForYou),
                Some(false) => Some(Origin::Query(query.unwrap_or_default())),
                None => None, // LEFT JOIN miss
            };
            let push_new = match out.last() {
                Some((p, _)) => p.id != id,
                None => true,
            };
            if push_new {
                let post = PostRow {
                    id: id.clone(),
                    handle: row.get("handle"),
                    created: row.get("created"),
                    likes: row.get::<i64, _>("likes") as u64,
                    retweets: row.get::<i64, _>("retweets") as u64,
                    replies: row.get::<i64, _>("replies") as u64,
                    impressions: row.get::<i64, _>("impressions") as u64,
                    seq: row.get("seq"),
                };
                out.push((post, Vec::new()));
            }
            if let Some(o) = origin {
                out.last_mut().unwrap().1.push(o);
            }
        }
        Ok(out)
    }

    /// Text/images/videos for each `post_id` from `contents`. Posts
    /// absent from `contents` (already reaped) are simply not in the map.
    pub async fn fetch_contents(
        &self,
        post_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, Vec<MediaUrl>, Vec<MediaUrl>)>, Error>
    {
        let mut out = std::collections::HashMap::with_capacity(post_ids.len());
        if post_ids.is_empty() {
            return Ok(out);
        }
        let rows = sqlx::query(
            "SELECT post_id, text, images, videos FROM contents WHERE post_id = ANY($1)",
        )
        .bind(post_ids)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            let id: String = row.get("post_id");
            let text: String = row.get("text");
            let images: Vec<MediaUrl> =
                serde_json::from_value(row.get("images")).unwrap_or_default();
            let videos: Vec<MediaUrl> =
                serde_json::from_value(row.get("videos")).unwrap_or_default();
            out.insert(id, (text, images, videos));
        }
        Ok(out)
    }

    /// Persisted `(score, handle)` for each `post_id`, in `ids` order.
    /// Missing rows fall back to `(0.0, "")`.
    pub async fn get_scored_handles(&self, ids: &[String]) -> Result<Vec<(f64, String)>, Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT p.id AS id, p.handle AS handle, COALESCE(s.score, 0.0) AS score
               FROM posts p
               LEFT JOIN scores s ON s.post_id = p.id
              WHERE p.id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        let mut by_id: std::collections::HashMap<String, (f64, String)> =
            std::collections::HashMap::new();
        for row in rows {
            let id: String = row.get("id");
            let handle: String = row.get("handle");
            let score: f64 = row.get("score");
            by_id.insert(id, (score, handle));
        }
        Ok(ids
            .iter()
            .map(|id| by_id.get(id).cloned().unwrap_or_else(|| (0.0, String::new())))
            .collect())
    }

    /// Upsert score rows and drop the matching `contents` row in one
    /// transaction. `ids` and `scores` must be the same length.
    pub async fn set_scores(&self, ids: &[String], scores: &[f64]) -> Result<(), Error> {
        assert_eq!(ids.len(), scores.len(), "ids/scores length mismatch");
        let mut tx = self.pool.begin().await?;
        for (id, score) in ids.iter().zip(scores.iter()) {
            sqlx::query(
                "INSERT INTO scores (post_id, score)
                 VALUES ($1, $2)
                 ON CONFLICT (post_id) DO UPDATE SET
                     score     = excluded.score,
                     scored_at = now()",
            )
            .bind(id)
            .bind(*score)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM contents WHERE post_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Enqueue a delivery (attempts=0). `target` + `post_ids` are JSON.
    pub async fn enqueue_delivery(
        &self,
        psyop: &str,
        target: &serde_json::Value,
        post_ids: &serde_json::Value,
    ) -> Result<i64, Error> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO delivery_queue (psyop, target, post_ids)
             VALUES ($1, $2, $3)
             RETURNING id",
        )
        .bind(psyop)
        .bind(target)
        .bind(post_ids)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// Reap `contents` for every post under `psyop`. Returns rows deleted.
    pub async fn drop_psyop_contents(&self, psyop: &str) -> Result<usize, Error> {
        let n = sqlx::query(
            "DELETE FROM contents
             WHERE post_id IN (SELECT id FROM posts WHERE psyop = $1)",
        )
        .bind(psyop)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n as usize)
    }

    /// All queued deliveries. `psyop_filter = Some(name)` narrows to one
    /// psyop; `None` returns every pending row.
    pub async fn list_pending_deliveries(
        &self,
        psyop_filter: Option<&str>,
    ) -> Result<Vec<QueuedDelivery>, Error> {
        let rows = sqlx::query(
            "SELECT id, psyop, target, post_ids, attempts, last_error, last_attempt_at
             FROM delivery_queue
             WHERE ($1::text IS NULL OR psyop = $1)
             ORDER BY id ASC",
        )
        .bind(psyop_filter)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| QueuedDelivery {
                id: r.get("id"),
                psyop: r.get("psyop"),
                target: r.get("target"),
                post_ids: r.get("post_ids"),
                attempts: r.get("attempts"),
                last_error: r.get("last_error"),
                last_attempt_at: r.get("last_attempt_at"),
            })
            .collect())
    }

    /// Bump `attempts`, set `last_error` + `last_attempt_at` (= now).
    pub async fn bump_delivery_attempt(&self, id: i64, last_error: &str) -> Result<(), Error> {
        sqlx::query(
            "UPDATE delivery_queue
             SET attempts = attempts + 1, last_error = $1, last_attempt_at = now()::text
             WHERE id = $2",
        )
        .bind(last_error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a queued delivery row.
    pub async fn delete_delivery(&self, id: i64) -> Result<(), Error> {
        sqlx::query("DELETE FROM delivery_queue WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

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
