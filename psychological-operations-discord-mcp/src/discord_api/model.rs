//! Agent-facing Discord data shapes. Mirrors the X MCP's slim-list /
//! rich-detail split: the list tools return the tiny [`MessageSummary`];
//! `get_message` returns the full [`MessageDetail`]. Every user reference is a
//! [`User`] (`user_id` + global `username`) so the agent always has the id the
//! per-user tools (`get_user`, `get_profile_picture`) key off.

use serde::Serialize;

/// A reference to a Discord user: the stable `user_id` plus the global
/// `username`. Used wherever a user appears (message author, `list_users`),
/// so the agent can both display the name and call the per-user tools by id.
#[derive(Debug, Clone, Serialize)]
pub(super) struct User {
    pub user_id: String,
    pub username: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AttachmentKind {
    Image,
    Video,
    File,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Attachment {
    pub kind: AttachmentKind,
    pub url: String,
}

/// `list_servers` item.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ServerInfo {
    pub id: String,
    pub name: String,
}

/// `get_user` result. `nickname` is the per-server nickname when a `server_id`
/// was given (falling back to `username` when unset), else just `username`.
#[derive(Debug, Clone, Serialize)]
pub(super) struct UserProfile {
    pub user: User,
    pub nickname: String,
    pub bot: bool,
}

/// `get_role` result. `color` is a `#RRGGBB` hex string (`#000000` = no color).
#[derive(Debug, Clone, Serialize)]
pub(super) struct RoleInfo {
    pub id: String,
    pub name: String,
    pub color: String,
    pub position: u16,
    /// Whether members with this role are shown separately in the member list.
    pub hoist: bool,
    pub mentionable: bool,
    /// Whether the role is managed by an integration/bot (not manually editable).
    pub managed: bool,
}

/// `list_channels` item. `kind` is the channel type (`text`, `news`,
/// `public_thread`, `forum`, …).
#[derive(Debug, Clone, Serialize)]
pub(super) struct ChannelInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Slim `list_messages` item — id, author (`user`), the reply target (if any),
/// the @-mentioned users, and the thread this message started (if any). Open it
/// with `get_message` for the full content; read a started thread with
/// `list_messages` on its `thread_channel_id`.
#[derive(Debug, Clone, Serialize)]
pub(super) struct MessageSummary {
    pub id: String,
    pub user: User,
    /// The id of the message this one replied to, if it's a reply. Fetch it with
    /// `get_message`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_to: Option<String>,
    /// The channel id of the thread started from this message, if any. Read it
    /// with `list_messages`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_channel_id: Option<String>,
}

/// Rich `get_message` detail.
#[derive(Debug, Clone, Serialize)]
pub(super) struct MessageDetail {
    pub id: String,
    pub user: User,
    pub content: String,
    pub attachments: Vec<Attachment>,
    /// The id of the message this one replied to, if it's a reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_to: Option<String>,
    /// The channel id of the thread started from this message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_channel_id: Option<String>,
    pub created_at: String,
}
