use serde::{Deserialize, Serialize};

/// Tiebreak order applied across the deduped candidate union when
/// truncating to `PsyOp.max_posts`.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    Likes,
    Retweets,
    Replies,
    Newest,
    Oldest,
}
