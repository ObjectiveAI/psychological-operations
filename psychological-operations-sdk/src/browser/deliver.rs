//! Reply/quote delivery request types.
//!
//! The CLI's `agents deliver` driver groups the queue by agent and spawns
//! one browser per agent in `--agent-deliver <tag> --items <json>` mode,
//! where `<json>` is an array of [`DeliverItem`]. The agent rides on the
//! invocation (the mode's tag), not on each item. The browser fulfills
//! each item as that agent and streams back one
//! [`crate::browser::output::Output::Delivered`] per success; the CLI
//! removes the matching `reply_quote_queue` row as each arrives.

use serde::{Deserialize, Serialize};

/// One queued reply/quote to deliver. The `--items` argument is a JSON
/// array of these; the acting agent is the `--agent-deliver` tag, not a
/// per-item field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverItem {
    /// The target tweet (the `in_reply_to_tweet_id` / `quote_tweet_id`).
    pub tweet_id: String,
    /// The reply/quote body.
    pub content: String,
    /// `"reply"` or `"quote"`.
    pub kind: String,
}
