//! Wire schema for the tweets the Chrome extension sends across the
//! native-messaging port, and the mapping into our canonical `db::Post`.

use serde::Deserialize;

use crate::db::{MediaUrl, Post};

/// One tweet as serialized by
/// `psychological-operations-chrome-extension/content_script.js`.
#[derive(Debug, Deserialize)]
pub struct IncomingTweet {
    pub id: String,
    pub handle: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub retweets: u64,
    #[serde(default)]
    pub replies: u64,
    #[serde(default)]
    pub images: Vec<MediaUrl>,
    #[serde(default)]
    pub videos: Vec<MediaUrl>,
}

impl IncomingTweet {
    /// Validate + convert. Returns `Err` for tweets that should be
    /// skipped (with a short reason for diagnostics — the native host
    /// counts but doesn't surface per-tweet reasons over the wire).
    pub fn into_post(self) -> Result<Post, &'static str> {
        if self.id.trim().is_empty() {
            return Err("missing id");
        }
        if self.handle.trim().is_empty() {
            return Err("missing handle");
        }
        if self.created.trim().is_empty() {
            return Err("missing created");
        }
        // Sanity-check the timestamp; we store it as the original ISO 8601
        // string (matching db::Post.created), but we want to reject
        // garbage.
        if chrono::DateTime::parse_from_rfc3339(&self.created).is_err() {
            return Err("created is not RFC 3339");
        }
        Ok(Post {
            id: self.id,
            handle: self.handle,
            text: self.text,
            images: self.images,
            videos: self.videos,
            created: self.created,
            likes: self.likes,
            retweets: self.retweets,
            replies: self.replies,
        })
    }
}
