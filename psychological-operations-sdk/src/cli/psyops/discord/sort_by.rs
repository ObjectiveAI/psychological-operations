use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tiebreak order applied to Discord messages within a priority bucket.
/// Discord messages carry no like/retweet/reply engagement metrics, so only
/// timestamp-based built-ins (plus a custom Python expression) are offered.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    Newest,
    Oldest,
    /// Python expression. The `input` global is a list of dicts mirroring a
    /// Discord message, in candidate order. Its trailing expression must
    /// evaluate to a list of **sort values** positionally aligned to `input`:
    /// element `i` is message `i`'s value. Messages sort **ascending** by
    /// value (equal values keep original order — negate for descending). An
    /// element of `None`, or a position past the end of a short list,
    /// **drops** that message; extra elements are ignored. Each element must
    /// be a number or `None`.
    Python(String),
}

impl SortBy {
    /// Publish-time check. The `Python` variant is not parse-checked
    /// here — errors surface at sort time.
    pub fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}
