//! Projection of the raw db DTOs into the agent-facing shapes in
//! [`super::model`]. (Twitch reads come straight from the postgres chat buffer,
//! so there's only the one row shape to reshape.)

use psychological_operations_db::TwitchMessage;

use super::model::MessageSummary;

/// Project a buffered [`TwitchMessage`] into the slim [`MessageSummary`] the
/// `list_messages` tool returns.
pub(super) fn message_summary(m: &TwitchMessage) -> MessageSummary {
    MessageSummary {
        message_id: m.message_id.clone(),
        user_login: m.user_login.clone(),
        user_id: m.user_id.clone(),
        content: m.content.clone(),
        sent_at: m.sent_at,
    }
}
