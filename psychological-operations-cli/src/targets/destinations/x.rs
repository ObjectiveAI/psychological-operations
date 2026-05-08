use serde::{Deserialize, Serialize};

use super::Subject;

/// "X" target — like or retweet each scored post on behalf of the
/// psyop's X account. The act-as user is determined per-psyop at
/// dispatch time; the next commit plumbs per-psyop OAuth tokens
/// (consuming `x_app.json`'s client_id / client_secret) and wires
/// the real API call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X {
    /// Internal field name uses raw-keyword `r#type` to mirror the
    /// user's spec; on the wire it serializes as `"action"` to avoid
    /// collision with the parent `Destination`'s `"type"` tag.
    #[serde(rename = "action")]
    pub r#type: XType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XType {
    Like,
    Retweet,
}

pub async fn send(_cfg: &X, _subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    Err(crate::error::Error::Other(
        "x target: per-psyop OAuth not yet authorized".into(),
    ))
}
