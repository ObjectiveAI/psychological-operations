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
                 handle, created, likes, retweets, replies)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                post.id, psyop, psyop_commit_sha,
                post.handle, post.created,
                post.likes as i64, post.retweets as i64, post.replies as i64,
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
