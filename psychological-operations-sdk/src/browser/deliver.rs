//! Reply/quote delivery request types.
//!
//! The CLI's `agents deliver` driver serializes a JSON array of
//! [`DeliverItem`] and spawns the browser with `--deliver <json>`. The
//! browser fulfills each item and streams back one
//! [`crate::browser::output::Output::Delivered`] per success (content
//! omitted); the CLI removes the matching `reply_quote_queue` row as each
//! arrives.

use serde::{Deserialize, Serialize};

/// One queued reply/quote to deliver. The browser's `--deliver` argument
/// is a JSON array of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverItem {
    /// The target tweet (the `in_reply_to_tweet_id` / `quote_tweet_id`).
    pub tweet_id: String,
    /// Agent tag to act as.
    pub agent: String,
    /// The reply/quote body.
    pub content: String,
    /// `"reply"` or `"quote"`.
    pub kind: String,
}
