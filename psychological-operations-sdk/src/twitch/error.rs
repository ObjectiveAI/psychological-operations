//! Errors for the Twitch client.

/// Failure modes of the Twitch [`Client`](super::client::Client). Mirrors the
/// Discord client's split: `NotAuthed` / `Db` / `Serde` are **system faults**
/// (setup / credentials / infra broke — not the agent's doing), while `Http`
/// carries an **agent-surfaceable** outcome of the authorized Helix request
/// (e.g. a bad status, or a chat message Twitch dropped).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No usable Twitch credentials: the agent has no `twitch_auth` access
    /// token, or there's no active Twitch app (client id). The string carries
    /// the specific reason. The agent must complete `agents login twitch`
    /// (and an operator must set the Twitch app credentials) first.
    #[error("twitch not authed: {0}")]
    NotAuthed(String),

    /// A database error while reading the agent's auth or the app credentials.
    #[error("twitch auth db error: {0}")]
    Db(#[from] psychological_operations_db::Error),

    /// (De)serializing a cached response body (Helix JSON ⇄ bytes in the
    /// response cache), or decoding a Helix response into our model.
    #[error("twitch (de)serialize error: {0}")]
    Serde(#[from] serde_json::Error),

    /// The authorized Helix / OAuth request's own outcome — a transport
    /// failure, a non-success status, or a dropped chat message. Agent-facing.
    #[error("twitch http error: {0}")]
    Http(String),
}

impl From<reqwest::Error> for Error {
    /// A transport failure (connect / read / body) reaching Twitch —
    /// surfaced to the agent like any other request outcome.
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.to_string())
    }
}
