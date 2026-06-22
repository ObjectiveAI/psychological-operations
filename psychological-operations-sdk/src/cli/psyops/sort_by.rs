use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tiebreak order applied to query tweets within a priority bucket
/// (for_you tweets interweave by arrival instead).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    Likes,
    Retweets,
    Replies,
    Newest,
    Oldest,
    /// Python expression. The `input` global is a list of dicts mirroring
    /// `Tweet` (keys: `id`, `handle`, `created`, `age`, `likes`, `retweets`,
    /// `replies`, `impressions`), in candidate order. Its trailing expression
    /// must evaluate to a list of **sort values** positionally aligned to
    /// `input`: element `i` is tweet `i`'s value. Tweets sort **ascending**
    /// by value (equal values keep original order — negate for descending).
    /// An element of `None`, or a position past the end of a short list,
    /// **drops** that tweet; extra elements are ignored. Each element must
    /// be a number or `None`.
    /// Example: `[t['likes'] if t['likes'] > 5 else None for t in input]`.
    Custom(String),
}

impl SortBy {
    /// Publish-time check. `Custom` is Python and is not parse-checked
    /// here — errors surface at sort time.
    pub fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}
