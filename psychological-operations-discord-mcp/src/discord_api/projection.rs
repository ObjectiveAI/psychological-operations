//! Projection: serenity model types → the agent-facing shapes in
//! [`super::model`]. `author` is the global username (`User.name`); the reply
//! target is the referenced message id; `mentions` are the @-mentioned users'
//! global usernames.

use psychological_operations_sdk::discord::serenity;
use serenity::all::{GuildChannel, Message};

use super::model::{Attachment, AttachmentKind, ChannelInfo, MessageDetail, MessageSummary};

/// Classify a Discord attachment by its `content_type`.
pub(super) fn attachment_kind(content_type: Option<&str>) -> AttachmentKind {
    match content_type {
        Some(ct) if ct.starts_with("image/") => AttachmentKind::Image,
        Some(ct) if ct.starts_with("video/") => AttachmentKind::Video,
        _ => AttachmentKind::File,
    }
}

fn collect_attachments(m: &Message) -> Vec<Attachment> {
    m.attachments
        .iter()
        .map(|a| Attachment {
            kind: attachment_kind(a.content_type.as_deref()),
            url: a.url.clone(),
        })
        .collect()
}

/// The id of the message this one replies to, if it's a reply.
fn replied_to(m: &Message) -> Option<String> {
    m.message_reference
        .as_ref()
        .and_then(|r| r.message_id)
        .map(|id| id.to_string())
}

/// Global usernames of the @-mentioned users.
fn mentions(m: &Message) -> Vec<String> {
    m.mentions.iter().map(|u| u.name.clone()).collect()
}

pub(super) fn project_message_summary(m: &Message) -> MessageSummary {
    MessageSummary {
        id: m.id.to_string(),
        author: m.author.name.clone(),
        replied_to: replied_to(m),
        mentions: mentions(m),
    }
}

pub(super) fn project_message_detail(m: &Message) -> MessageDetail {
    MessageDetail {
        id: m.id.to_string(),
        author: m.author.name.clone(),
        content: m.content.clone(),
        attachments: collect_attachments(m),
        replied_to: replied_to(m),
        mentions: mentions(m),
        created_at: m.timestamp.to_string(),
    }
}

pub(super) fn channel_info(c: &GuildChannel) -> ChannelInfo {
    ChannelInfo {
        id: c.id.to_string(),
        name: c.name.clone(),
        kind: c.kind.name().to_string(),
    }
}
