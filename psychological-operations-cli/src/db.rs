use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};

use crate::config;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS posts (
        id                TEXT    NOT NULL,
        psyop             TEXT    NOT NULL,
        psyop_commit_sha  TEXT    NOT NULL,
        handle            TEXT    NOT NULL,
        created           TEXT    NOT NULL,
        likes             INTEGER NOT NULL DEFAULT 0,
        retweets          INTEGER NOT NULL DEFAULT 0,
        replies           INTEGER NOT NULL DEFAULT 0,
        impressions       INTEGER NOT NULL DEFAULT 0,
        ingested_at       TEXT    NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (id, psyop, psyop_commit_sha)
    );
    CREATE INDEX IF NOT EXISTS posts_by_psyop ON posts(psyop, psyop_commit_sha);

    CREATE TABLE IF NOT EXISTS sources (
        post_id     TEXT    NOT NULL,
        for_you     INTEGER NOT NULL,
        query       TEXT,
        sourced_at  TEXT    NOT NULL DEFAULT (datetime('now')),
        CHECK (
            (for_you = 1 AND query IS NULL)
         OR (for_you = 0 AND query IS NOT NULL)
        )
    );
    CREATE UNIQUE INDEX IF NOT EXISTS sources_unique
        ON sources(post_id, COALESCE(query, ''));

    CREATE TABLE IF NOT EXISTS contents (
        post_id  TEXT PRIMARY KEY,
        text     TEXT NOT NULL,
        images   TEXT NOT NULL DEFAULT '[]',
        videos   TEXT NOT NULL DEFAULT '[]'
    );

    CREATE TABLE IF NOT EXISTS scores (
        post_id    TEXT PRIMARY KEY,
        score      REAL NOT NULL,
        scored_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE IF NOT EXISTS for_you_queue (
        post_id           TEXT NOT NULL,
        psyop             TEXT NOT NULL,
        psyop_commit_sha  TEXT NOT NULL,
        ingested_at       TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (post_id, psyop, psyop_commit_sha)
    );
    CREATE INDEX IF NOT EXISTS for_you_queue_by_psyop
        ON for_you_queue(psyop, psyop_commit_sha);

    CREATE TABLE IF NOT EXISTS delivery_queue (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        psyop             TEXT    NOT NULL,
        psyop_commit_sha  TEXT    NOT NULL,
        target_json       TEXT    NOT NULL,
        post_ids_json     TEXT    NOT NULL,
        attempts          INTEGER NOT NULL DEFAULT 0,
        last_error        TEXT,
        last_attempt_at   TEXT,
        created_at        TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS delivery_queue_by_psyop
        ON delivery_queue(psyop, psyop_commit_sha);
";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaUrl {
    pub url: String,
}

/// Canonical tweet content + engagement metadata.
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

pub struct Db {
    pool: SqlitePool,
}

/// Parse `created` (RFC 3339) and return seconds since `now`. A
/// `created` that doesn't parse yields 0 — `min_age` filters
/// would reject it anyway, and we'd rather not error out the whole
/// runtime over one bad timestamp.
pub(crate) fn compute_age(created: &str, now: &chrono::DateTime<chrono::Utc>) -> u64 {
    match chrono::DateTime::parse_from_rfc3339(created) {
        Ok(t) => {
            let secs = (*now - t.with_timezone(&chrono::Utc)).num_seconds();
            secs.max(0) as u64
        }
        Err(_) => 0,
    }
}

impl Db {
    pub async fn open(cfg: &crate::run::Config) -> Result<Self, crate::error::Error> {
        let path = config::db_path(cfg);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(30));
        let pool = SqlitePoolOptions::new()
            .max_connections(100)
            .connect_with(opts)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Ingest a post under `(psyop, psyop_commit_sha)` with the given
    /// origin.
    ///
    /// If the post already has a row in `scores`, the entire ingestion
    /// is a no-op — no posts row, no source row, no contents row.
    /// (Once a tweet has been scored, re-ingesting it would just churn
    /// rows back through scoring; the score is the authoritative
    /// answer.) Returns `Ok(false)` in this case.
    ///
    /// Otherwise, three things happen in one transaction:
    ///
    ///   1. **posts** — insert-or-ignore. If a row already exists for
    ///      this `(id, psyop, psyop_commit_sha)`, the existing row's
    ///      engagement counts and `ingested_at` are kept (first
    ///      observation wins).
    ///   2. **sources** — insert-or-ignore. A row is added for this
    ///      post + origin if one isn't already present, so a tweet
    ///      that arrives via multiple inputs (for_you AND a query, or
    ///      via two distinct queries) is tagged with each source.
    ///   3. **contents** — insert-or-ignore. If the row is already
    ///      present, the existing text/media wins (first observation).
    ///      If it's missing this re-ingestion re-adds the contents
    ///      alongside the new source row.
    ///
    /// Returns `true` if a *new source* row was created, `false`
    /// otherwise (already-scored, or already-ingested via this same
    /// origin). The post-row creation status is intentionally not
    /// surfaced — multi-source posts shouldn't be reported as
    /// "skipped" just because the post itself was already known.
    pub async fn insert_post(
        &self,
        post: &Post,
        psyop: &str,
        psyop_commit_sha: &str,
        origin: &Origin,
    ) -> Result<bool, crate::error::Error> {
        let (for_you, query) = match origin {
            Origin::ForYou => (1_i64, None),
            Origin::Query(q) => (0_i64, Some(q.as_str())),
        };
        let mut tx = self.pool.begin().await?;

        // Already scored? Skip everything. The SELECT runs inside the
        // transaction so we observe a consistent snapshot.
        let already_scored: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM scores WHERE post_id = ? LIMIT 1",
        )
        .bind(&post.id)
        .fetch_optional(&mut *tx)
        .await?;
        if already_scored.is_some() {
            tx.commit().await?;
            return Ok(false);
        }

        sqlx::query(
            "INSERT OR IGNORE INTO posts
                (id, psyop, psyop_commit_sha,
                 handle, created, likes, retweets, replies, impressions)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&post.id)
        .bind(psyop)
        .bind(psyop_commit_sha)
        .bind(&post.handle)
        .bind(&post.created)
        .bind(post.likes as i64)
        .bind(post.retweets as i64)
        .bind(post.replies as i64)
        .bind(post.impressions as i64)
        .execute(&mut *tx)
        .await?;

        let source_inserted = sqlx::query(
            "INSERT OR IGNORE INTO sources (post_id, for_you, query)
             VALUES (?, ?, ?)",
        )
        .bind(&post.id)
        .bind(for_you)
        .bind(query)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        let images_json = serde_json::to_string(&post.images)?;
        let videos_json = serde_json::to_string(&post.videos)?;
        sqlx::query(
            "INSERT OR IGNORE INTO contents (post_id, text, images, videos)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&post.id)
        .bind(&post.text)
        .bind(&images_json)
        .bind(&videos_json)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(source_inserted)
    }

    /// Browser-extension entry point. Queues a `(post_id, psyop,
    /// psyop_commit_sha)` triple for later runtime hydration. The
    /// queue exists because the Chromium extension's "Capture" only
    /// notes "I saw this id in for-you"; the actual posts/sources/
    /// contents rows are written by the runtime after fetching the
    /// tweet from the X v2 API.
    ///
    /// `INSERT OR IGNORE` — duplicate triples are silently coalesced.
    /// Returns `true` iff a new queue row was created.
    pub async fn enqueue_for_you(
        &self,
        post_id: &str,
        psyop: &str,
        psyop_commit_sha: &str,
    ) -> Result<bool, crate::error::Error> {
        let n = sqlx::query(
            "INSERT OR IGNORE INTO for_you_queue
                (post_id, psyop, psyop_commit_sha)
             VALUES (?, ?, ?)",
        )
        .bind(post_id)
        .bind(psyop)
        .bind(psyop_commit_sha)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n > 0)
    }

    /// Runtime entry point. Returns every queued post_id for this
    /// `(psyop, psyop_commit_sha)` ordered by `ingested_at ASC` so
    /// older observations get hydrated first.
    pub async fn for_you_queue(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
    ) -> Result<Vec<String>, crate::error::Error> {
        let out: Vec<String> = sqlx::query_scalar(
            "SELECT post_id FROM for_you_queue
              WHERE psyop = ? AND psyop_commit_sha = ?
              ORDER BY ingested_at ASC",
        )
        .bind(psyop)
        .bind(psyop_commit_sha)
        .fetch_all(&self.pool)
        .await?;
        Ok(out)
    }

    /// Runtime entry point. Drops queue rows AFTER the runtime has
    /// successfully hydrated them via `insert_post`. Caller passes
    /// only the ids it actually persisted, so a partial X-API
    /// failure leaves the rest in the queue for the next round.
    pub async fn dequeue_for_you(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
        post_ids: &[String],
    ) -> Result<(), crate::error::Error> {
        if post_ids.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for id in post_ids {
            sqlx::query(
                "DELETE FROM for_you_queue
                  WHERE post_id = ?
                    AND psyop = ?
                    AND psyop_commit_sha = ?",
            )
            .bind(id)
            .bind(psyop)
            .bind(psyop_commit_sha)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Runtime read path for filter / sort / score. Returns every
    /// posts row for this `(psyop, psyop_commit_sha)` that doesn't
    /// have a matching scores row, paired with all of its origins
    /// (every `sources` row that shares post_id). `now` is used to
    /// compute each tweet's `age` once at fetch time.
    ///
    /// `LEFT JOIN` against sources keeps tweets that somehow have no
    /// source row from being silently dropped — every tweet *should*
    /// have at least one source, but we don't bet runtime
    /// correctness on it.
    pub async fn list_unscored_with_origins(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
        now: &chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(crate::tweet::Tweet, Vec<Origin>, i64)>, crate::error::Error> {
        let rows = sqlx::query(
            "SELECT
                p.id, p.handle, p.created,
                p.likes, p.retweets, p.replies, p.impressions,
                s.for_you, s.query,
                p.rowid AS rowid
             FROM posts p
             LEFT JOIN sources s ON s.post_id = p.id
             WHERE p.psyop = ?
               AND p.psyop_commit_sha = ?
               AND NOT EXISTS (
                 SELECT 1 FROM scores sc WHERE sc.post_id = p.id
               )
             ORDER BY p.id",
        )
        .bind(psyop)
        .bind(psyop_commit_sha)
        .fetch_all(&self.pool)
        .await?;

        // Collapse the row stream into one (Tweet, Vec<Origin>, rowid)
        // per post id. The query is ORDER BY p.id so all rows for
        // one post arrive contiguously — a single-pass walk works.
        // `rowid` is the post's insertion order in `posts`; for
        // for_you-origin tweets that corresponds to browser-arrival
        // order via `hydrate_for_you`'s queue-order traversal.
        let mut out: Vec<(crate::tweet::Tweet, Vec<Origin>, i64)> = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            let handle: String = row.get("handle");
            let created: String = row.get("created");
            let likes: i64 = row.get("likes");
            let retweets: i64 = row.get("retweets");
            let replies: i64 = row.get("replies");
            let impressions: i64 = row.get("impressions");
            let for_you: Option<i64> = row.get("for_you");
            let query: Option<String> = row.get("query");
            let rowid: i64 = row.get("rowid");

            let origin = match for_you {
                Some(1) => Some(Origin::ForYou),
                Some(0) => Some(Origin::Query(query.unwrap_or_default())),
                _ => None, // LEFT JOIN miss
            };
            let age = compute_age(&created, now);
            let push_new = match out.last() {
                Some((t, _, _)) => t.id != id,
                None => true,
            };
            if push_new {
                let tweet = crate::tweet::Tweet {
                    id: id.clone(),
                    handle,
                    created,
                    age,
                    likes:       likes       as u64,
                    retweets:    retweets    as u64,
                    replies:     replies     as u64,
                    impressions: impressions as u64,
                };
                out.push((tweet, Vec::new(), rowid));
            }
            if let Some(o) = origin {
                out.last_mut().unwrap().1.push(o);
            }
        }
        Ok(out)
    }

    /// Look up text/images/videos for each `post_id` from `contents`.
    /// Posts whose row is absent from `contents` (already reaped by
    /// a prior `set_scores`, or for any reason missing) are simply
    /// not in the returned map. Callers treat absence as "this post
    /// doesn't exist for our purposes" — the runtime filters the
    /// post out of scoring rather than substituting empty content.
    ///
    /// Batches SELECTs in chunks of 500 to stay well under SQLite's
    /// default 999-bind-param cap.
    pub async fn fetch_contents(
        &self,
        post_ids: &[String],
    ) -> Result<
        std::collections::HashMap<String, (String, Vec<MediaUrl>, Vec<MediaUrl>)>,
        crate::error::Error,
    > {
        let mut out: std::collections::HashMap<
            String,
            (String, Vec<MediaUrl>, Vec<MediaUrl>),
        > = std::collections::HashMap::with_capacity(post_ids.len());
        if post_ids.is_empty() {
            return Ok(out);
        }
        const CHUNK: usize = 500;
        for chunk in post_ids.chunks(CHUNK) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT post_id, text, images, videos FROM contents WHERE post_id IN (",
            );
            {
                let mut sep = qb.separated(", ");
                for id in chunk {
                    sep.push_bind(id);
                }
            }
            qb.push(")");
            let rows = qb.build().fetch_all(&self.pool).await?;
            for row in rows {
                let id: String = row.get("post_id");
                let text: String = row.get("text");
                let images_json: String = row.get("images");
                let videos_json: String = row.get("videos");
                let images: Vec<MediaUrl> =
                    serde_json::from_str(&images_json).unwrap_or_default();
                let videos: Vec<MediaUrl> =
                    serde_json::from_str(&videos_json).unwrap_or_default();
                out.insert(id, (text, images, videos));
            }
        }
        Ok(out)
    }

    /// Look up the persisted score + handle for each `post_id`, in
    /// the same order as `ids`. Joins `posts` + `scores` so a single
    /// query backs the delivery worker's score rehydration and its URL
    /// formatting (handle goes into
    /// `https://x.com/<handle>/status/<id>`). Missing rows fall back
    /// to `(0.0, "")`.
    pub async fn get_scored_handles(
        &self,
        ids: &[String],
    ) -> Result<Vec<(f64, String)>, crate::error::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT p.id AS id, p.handle AS handle, COALESCE(s.score, 0.0) AS score \
               FROM posts p \
               LEFT JOIN scores s ON s.post_id = p.id \
              WHERE p.id IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for id in ids {
                sep.push_bind(id);
            }
        }
        qb.push(")");
        let rows = qb.build().fetch_all(&self.pool).await?;
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

    /// Upsert score rows keyed by `post_id` and drop the matching
    /// `contents` row in the same transaction — once a post has a
    /// score, its raw text/media is no longer needed. The (psyop,
    /// commit) context isn't repeated on the scores row; it's
    /// recoverable via the matching `posts` row. `ids` and `scores`
    /// must be the same length.
    pub async fn set_scores(
        &self,
        ids: &[String],
        scores: &[f64],
    ) -> Result<(), crate::error::Error> {
        assert_eq!(ids.len(), scores.len(), "ids/scores length mismatch");
        let mut tx = self.pool.begin().await?;
        for (id, score) in ids.iter().zip(scores.iter()) {
            sqlx::query(
                "INSERT INTO scores (post_id, score)
                 VALUES (?, ?)
                 ON CONFLICT(post_id) DO UPDATE SET
                     score     = excluded.score,
                     scored_at = datetime('now')",
            )
            .bind(id)
            .bind(*score)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM contents WHERE post_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Enqueue a delivery up front (attempts=0, no error yet). The
    /// runtime uses this after scoring so every applicable target
    /// gets a queued row, and `targets deliver` (a.k.a.
    /// `drain_queue`) sweeps them uniformly.
    pub async fn enqueue_delivery(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
        target_json: &str,
        post_ids_json: &str,
    ) -> Result<i64, crate::error::Error> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO delivery_queue
                (psyop, psyop_commit_sha, target_json, post_ids_json)
             VALUES (?, ?, ?, ?)
             RETURNING id",
        )
        .bind(psyop)
        .bind(psyop_commit_sha)
        .bind(target_json)
        .bind(post_ids_json)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// Reap `contents` for every post under (psyop, commit). Idempotent;
    /// safe after `set_scores` (which already drops content for the
    /// scored ids — this catches unscored posts whose content would
    /// otherwise leak forever). Returns the number of rows deleted.
    pub async fn drop_psyop_contents(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
    ) -> Result<usize, crate::error::Error> {
        let n = sqlx::query(
            "DELETE FROM contents
             WHERE post_id IN (
                 SELECT id FROM posts
                 WHERE psyop = ? AND psyop_commit_sha = ?
             )",
        )
        .bind(psyop)
        .bind(psyop_commit_sha)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n as usize)
    }

    /// Returns all queued (not-yet-redelivered) rows.
    /// `psyop_filter = Some(name)` narrows to one psyop;
    /// `commit_filter = Some(sha)` narrows to one commit. Both
    /// are independent; `None` on either is a no-op.
    pub async fn list_pending_deliveries(
        &self,
        psyop_filter:  Option<&str>,
        commit_filter: Option<&str>,
    ) -> Result<Vec<QueuedDelivery>, crate::error::Error> {
        // Positional `?` can't be reused, so each filter is bound
        // twice — once for the `IS NULL` guard, once for the match.
        let rows = sqlx::query(
            "SELECT id, psyop, psyop_commit_sha, target_json, post_ids_json,
                    attempts, last_error, last_attempt_at
             FROM delivery_queue
             WHERE (? IS NULL OR psyop            = ?)
               AND (? IS NULL OR psyop_commit_sha = ?)
             ORDER BY id ASC",
        )
        .bind(psyop_filter)
        .bind(psyop_filter)
        .bind(commit_filter)
        .bind(commit_filter)
        .fetch_all(&self.pool)
        .await?;
        let out = rows
            .into_iter()
            .map(|r| QueuedDelivery {
                id:               r.get("id"),
                psyop:            r.get("psyop"),
                psyop_commit_sha: r.get("psyop_commit_sha"),
                target_json:      r.get("target_json"),
                post_ids_json:    r.get("post_ids_json"),
                attempts:         r.get("attempts"),
                last_error:       r.get("last_error"),
                last_attempt_at:  r.get("last_attempt_at"),
            })
            .collect();
        Ok(out)
    }

    /// Bump `attempts`, update `last_error` + `last_attempt_at`.
    /// Use when a retry attempt for a queued row fails again.
    pub async fn bump_delivery_attempt(
        &self,
        id: i64,
        last_error: &str,
    ) -> Result<(), crate::error::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE delivery_queue
             SET attempts        = attempts + 1,
                 last_error      = ?,
                 last_attempt_at = ?
             WHERE id = ?",
        )
        .bind(last_error)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a queued delivery row (delivered successfully, or
    /// operator-pruned).
    pub async fn delete_delivery(&self, id: i64) -> Result<(), crate::error::Error> {
        sqlx::query("DELETE FROM delivery_queue WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// One row from `delivery_queue` — a delivery awaiting (re)delivery.
/// `last_error` / `last_attempt_at` are `None` for freshly enqueued
/// rows that haven't been attempted yet; `Some(...)` after at least
/// one failed attempt.
#[derive(Debug, Clone)]
pub struct QueuedDelivery {
    pub id:               i64,
    pub psyop:            String,
    pub psyop_commit_sha: String,
    pub target_json:      String,
    pub post_ids_json:    String,
    pub attempts:         i64,
    pub last_error:       Option<String>,
    pub last_attempt_at:  Option<String>,
}
