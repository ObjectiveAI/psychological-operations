use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// "X" target — like or retweet each scored post on behalf of the
/// psyop's X account. The acting user is determined per-psyop via
/// the OAuth tokens at `~/.psychological-operations/tokens/<name>.json`,
/// silently refreshed if expired.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct X {
    /// Internal field name uses raw-keyword `r#type` to mirror the
    /// user's spec; on the wire it serializes as `"action"` to avoid
    /// collision with the parent `Destination`'s `"type"` tag.
    #[serde(rename = "action")]
    pub r#type: XType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum XType {
    Like,
    Retweet,
}
