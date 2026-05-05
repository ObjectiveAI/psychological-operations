use std::time::Duration;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::config;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS posts (
        id                TEXT    NOT NULL,
        psyop             TEXT    NOT NULL,
        psyop_commit_sha  TEXT    NOT NULL,
        for_you           INTEGER NOT NULL,
        query             TEXT,
        handle            TEXT    NOT NULL,
        created           TEXT    NOT NULL,
        likes             INTEGER NOT NULL DEFAULT 0,
        retweets          INTEGER NOT NULL DEFAULT 0,
        replies           INTEGER NOT NULL DEFAULT 0,
        ingested_at       TEXT    NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (id, psyop, psyop_commit_sha),
        CHECK (
            (for_you = 1 AND query IS NULL)
         OR (for_you = 0 AND query IS NOT NULL)
        )
    );
    CREATE INDEX IF NOT EXISTS posts_by_psyop ON posts(psyop, psyop_commit_sha);

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
/// `(for_you, query)` column pair on the `posts` table.
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

    /// Insert one `(post, psyop, psyop_commit_sha, origin)` row and
    /// upsert the post's content (keyed by post id alone, so the same
    /// tweet ingested under multiple psyops doesn't duplicate). Returns
    /// `true` if a new posts row was created, `false` if the
    /// `(id, psyop, psyop_commit_sha)` triple already existed.
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
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO posts
                (id, psyop, psyop_commit_sha, for_you, query,
                 handle, created, likes, retweets, replies)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                post.id, psyop, psyop_commit_sha, for_you, query,
                post.handle, post.created,
                post.likes as i64, post.retweets as i64, post.replies as i64,
            ],
        )? > 0;
        let images_json = serde_json::to_string(&post.images)?;
        let videos_json = serde_json::to_string(&post.videos)?;
        tx.execute(
            "INSERT INTO contents (post_id, text, images, videos)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(post_id) DO UPDATE SET
                 text = excluded.text,
                 images = excluded.images,
                 videos = excluded.videos",
            params![post.id, post.text, images_json, videos_json],
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Upsert score rows keyed by `post_id`. The (psyop, commit)
    /// context isn't repeated here — it's recoverable via the matching
    /// `posts` row. `ids` and `scores` must be the same length.
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
        }
        tx.commit()?;
        Ok(())
    }
}
