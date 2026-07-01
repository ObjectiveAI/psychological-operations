//! Agent-facing Twitch data shapes.

use serde::Serialize;

/// `whoami` result — the Twitch identity the agent acts as.
#[derive(Debug, Clone, Serialize)]
pub(super) struct WhoAmI {
    pub user_id: String,
    pub login: String,
}

/// Slim `list_messages` item — one buffered chat message. `sent_at` is unix
/// seconds (when the daemon received it).
#[derive(Debug, Clone, Serialize)]
pub(super) struct MessageSummary {
    pub message_id: String,
    pub user_login: String,
    pub user_id: String,
    pub content: String,
    pub sent_at: i64,
}

/// `send_message` result — the channel it landed in and the new message id.
#[derive(Debug, Clone, Serialize)]
pub(super) struct SentMessage {
    pub channel: String,
    pub message_id: String,
}
