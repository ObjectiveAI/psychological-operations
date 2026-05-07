use std::time::Duration;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

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
    conn: Connection,
}

/// Parse `created` (RFC 3339) and return seconds since `now`. A
/// `created` that doesn't parse yields 0 — `min_age` filters
/// would reject it anyway, and we'd rather not error out the whole
/// runtime over one bad timestamp.
fn compute_age(created: &str, now: &chrono::DateTime<chrono::Utc>) -> u64 {
    match chrono::DateTime::parse_from_rfc3339(created) {
        Ok(t) => {
            let secs = (*now - t.with_timezone(&chrono::Utc)).num_seconds();
            secs.max(0) as u64
        }
        Err(_) => 0,
    }
}

impl Db {
    pub fn open() -> Result<Self, crate::error::Error> {
        let path = config::db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
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
    pub fn insert_post(
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
        let tx = self.conn.unchecked_transaction()?;

        // Already scored? Skip everything. Use SELECT 1 ... LIMIT 1
        // inside the transaction so we observe a consistent snapshot.
        let already_scored: bool = tx
            .query_row(
                "SELECT 1 FROM scores WHERE post_id = ?1 LIMIT 1",
                params![post.id],
                |_| Ok(true),
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                other => Err(other),
            })?;
        if already_scored {
            tx.commit()?;
            return Ok(false);
        }

        tx.execute(
            "INSERT OR IGNORE INTO posts
                (id, psyop, psyop_commit_sha,
                 handle, created, likes, retweets, replies, impressions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                post.id, psyop, psyop_commit_sha,
                post.handle, post.created,
                post.likes as i64, post.retweets as i64, post.replies as i64,
                post.impressions as i64,
            ],
        )?;

        let source_inserted = tx.execute(
            "INSERT OR IGNORE INTO sources (post_id, for_you, query)
             VALUES (?1, ?2, ?3)",
            params![post.id, for_you, query],
        )? > 0;

        let images_json = serde_json::to_string(&post.images)?;
        let videos_json = serde_json::to_string(&post.videos)?;
        tx.execute(
            "INSERT OR IGNORE INTO contents (post_id, text, images, videos)
             VALUES (?1, ?2, ?3, ?4)",
            params![post.id, post.text, images_json, videos_json],
        )?;

        tx.commit()?;
        Ok(source_inserted)
    }

    /// Browser-extension entry point. Queues a `(post_id, psyop,
    /// psyop_commit_sha)` triple for later runtime hydration. The
    /// queue exists because the chrome extension's "Capture" only
    /// notes "I saw this id in for-you"; the actual posts/sources/
    /// contents rows are written by the runtime after fetching the
    /// tweet from the X v2 API.
    ///
    /// `INSERT OR IGNORE` — duplicate triples are silently coalesced.
    /// Returns `true` iff a new queue row was created.
    pub fn enqueue_for_you(
        &self,
        post_id: &str,
        psyop: &str,
        psyop_commit_sha: &str,
    ) -> Result<bool, crate::error::Error> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO for_you_queue
                (post_id, psyop, psyop_commit_sha)
             VALUES (?1, ?2, ?3)",
            params![post_id, psyop, psyop_commit_sha],
        )?;
        Ok(n > 0)
    }

    /// Runtime entry point. Returns every queued post_id for this
    /// `(psyop, psyop_commit_sha)` ordered by `ingested_at ASC` so
    /// older observations get hydrated first.
    pub fn for_you_queue(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
    ) -> Result<Vec<String>, crate::error::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT post_id FROM for_you_queue
              WHERE psyop = ?1 AND psyop_commit_sha = ?2
              ORDER BY ingested_at ASC",
        )?;
        let rows = stmt.query_map(params![psyop, psyop_commit_sha], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Runtime entry point. Drops queue rows AFTER the runtime has
    /// successfully hydrated them via `insert_post`. Caller passes
    /// only the ids it actually persisted, so a partial X-API
    /// failure leaves the rest in the queue for the next round.
    pub fn dequeue_for_you(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
        post_ids: &[String],
    ) -> Result<(), crate::error::Error> {
        if post_ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for id in post_ids {
            tx.execute(
                "DELETE FROM for_you_queue
                  WHERE post_id = ?1
                    AND psyop = ?2
                    AND psyop_commit_sha = ?3",
                params![id, psyop, psyop_commit_sha],
            )?;
        }
        tx.commit()?;
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
    pub fn list_unscored_with_origins(
        &self,
        psyop: &str,
        psyop_commit_sha: &str,
        now: &chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(crate::tweet::Tweet, Vec<Origin>)>, crate::error::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT
                p.id, p.handle, p.created,
                p.likes, p.retweets, p.replies, p.impressions,
                s.for_you, s.query
             FROM posts p
             LEFT JOIN sources s ON s.post_id = p.id
             WHERE p.psyop = ?1
               AND p.psyop_commit_sha = ?2
               AND NOT EXISTS (
                 SELECT 1 FROM scores sc WHERE sc.post_id = p.id
               )
             ORDER BY p.id",
        )?;

        struct Row {
            id: String,
            handle: String,
            created: String,
            likes: i64,
            retweets: i64,
            replies: i64,
            impressions: i64,
            origin: Option<Origin>,
        }

        let rows = stmt.query_map(params![psyop, psyop_commit_sha], |row| {
            let for_you: Option<i64> = row.get(7)?;
            let query: Option<String> = row.get(8)?;
            let origin = match for_you {
                Some(1) => Some(Origin::ForYou),
                Some(0) => Some(Origin::Query(query.unwrap_or_default())),
                _       => None,   // LEFT JOIN miss
            };
            Ok(Row {
                id:          row.get(0)?,
                handle:      row.get(1)?,
                created:     row.get(2)?,
                likes:       row.get(3)?,
                retweets:    row.get(4)?,
                replies:     row.get(5)?,
                impressions: row.get(6)?,
                origin,
            })
        })?;

        // Collapse the row stream into one (Tweet, Vec<Origin>) per
        // post id. The query is ORDER BY p.id so all rows for one
        // post arrive contiguously — a single-pass walk works.
        let mut out: Vec<(crate::tweet::Tweet, Vec<Origin>)> = Vec::new();
        for r in rows {
            let r = r?;
            let age = compute_age(&r.created, now);
            let push_new = match out.last() {
                Some((t, _)) => t.id != r.id,
                None => true,
            };
            if push_new {
                let tweet = crate::tweet::Tweet {
                    id: r.id.clone(),
                    handle: r.handle,
                    created: r.created,
                    age,
                    likes:       r.likes       as u64,
                    retweets:    r.retweets    as u64,
                    replies:     r.replies     as u64,
                    impressions: r.impressions as u64,
                };
                out.push((tweet, Vec::new()));
            }
            if let Some(o) = r.origin {
                out.last_mut().unwrap().1.push(o);
            }
        }
        Ok(out)
    }

    /// Upsert score rows keyed by `post_id` and drop the matching
    /// `contents` row in the same transaction — once a post has a
    /// score, its raw text/media is no longer needed. The (psyop,
    /// commit) context isn't repeated on the scores row; it's
    /// recoverable via the matching `posts` row. `ids` and `scores`
    /// must be the same length.
    pub fn set_scores(
        &self,
        ids: &[String],
        scores: &[f64],
    ) -> Result<(), crate::error::Error> {
        assert_eq!(ids.len(), scores.len(), "ids/scores length mismatch");
        let tx = self.conn.unchecked_transaction()?;
        for (id, score) in ids.iter().zip(scores.iter()) {
            tx.execute(
                "INSERT INTO scores (post_id, score)
                 VALUES (?1, ?2)
                 ON CONFLICT(post_id) DO UPDATE SET
                     score     = excluded.score,
                     scored_at = datetime('now')",
                params![id, score],
            )?;
            tx.execute(
                "DELETE FROM contents WHERE post_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
