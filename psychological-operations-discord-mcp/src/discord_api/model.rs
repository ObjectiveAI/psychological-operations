//! Agent-facing Discord data shapes. Mirrors the X MCP's slim-list /
//! rich-detail split: the list tools return the tiny [`MessageSummary`];
//! `get_message` returns the full [`MessageDetail`]. `author` is always the
//! global Discord username (not the per-server nickname).

use serde::Serialize;

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

/// `list_channels` item. `kind` is the channel type (`text`, `news`,
/// `public_thread`, `forum`, …).
#[derive(Debug, Clone, Serialize)]
pub(super) struct ChannelInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Slim `list_messages` item — id, author, the reply target (if any), and the
/// @-mentioned users. Open it with `get_message` for the full content.
#[derive(Debug, Clone, Serialize)]
pub(super) struct MessageSummary {
    pub id: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
}

/// Rich `get_message` detail.
#[derive(Debug, Clone, Serialize)]
pub(super) struct MessageDetail {
    pub id: String,
    pub author: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    pub created_at: String,
}
