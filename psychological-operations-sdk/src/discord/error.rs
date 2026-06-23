//! Errors for the Discord client.

/// Failure modes of the Discord [`Client`](super::client::Client): resolving
/// the agent's bot token from the DB, or building the gateway client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No `discord_auth` row for the agent, or the row has no `bot_token`.
    /// The agent must complete `agents login discord` first.
    #[error("agent '{0}' has no Discord bot token — run `agents login discord` first")]
    NotAuthed(String),

    /// A database error while reading the agent's auth.
    #[error("discord auth db error: {0}")]
    Db(#[from] psychological_operations_db::Error),

    /// A serenity error building the gateway client.
    #[error("serenity error: {0}")]
    Serenity(#[from] serenity::Error),

    /// (De)serializing a cached response body (serenity model ⇄ JSON bytes in
    /// the response cache).
    #[error("discord cache (de)serialize error: {0}")]
    Serde(#[from] serde_json::Error),
}
