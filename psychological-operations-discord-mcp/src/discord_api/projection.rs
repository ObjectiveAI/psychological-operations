//! Projection: serenity model types → the agent-facing shapes in
//! [`super::model`]. `author` is the global username (`User.name`); the reply
//! target is the referenced message id; `mentions` are the @-mentioned users'
//! global usernames.

use psychological_operations_sdk::discord::serenity;
use serenity::all::{GuildChannel, Message};

use super::model::{Attachment, AttachmentKind, ChannelInfo, MessageDetail, MessageSummary, User};

/// A [`User`] reference (`user_id` + global username) from a serenity user.
pub(super) fn user_ref(u: &serenity::all::User) -> User {
    User {
        user_id: u.id.to_string(),
        username: u.name.clone(),
    }
}

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

/// The user this message replied to, if it's a reply — taken from the resolved
/// referenced message (which `get_messages`/`get_message` carry inline). `None`
/// when not a reply, or when the referenced message wasn't resolved/was deleted.
fn replied_to(m: &Message) -> Option<User> {
    m.referenced_message.as_ref().map(|r| user_ref(&r.author))
}

/// Global usernames of the @-mentioned users.
fn mentions(m: &Message) -> Vec<String> {
    m.mentions.iter().map(|u| u.name.clone()).collect()
}

/// The channel id of the thread started from this message, if any. Carried
/// inline on the message by `get_messages`/`get_message` — no extra fetch.
fn thread_id(m: &Message) -> Option<String> {
    m.thread.as_ref().map(|t| t.id.to_string())
}

pub(super) fn project_message_summary(m: &Message) -> MessageSummary {
    MessageSummary {
        id: m.id.to_string(),
        user: user_ref(&m.author),
        replied_to: replied_to(m),
        mentions: mentions(m),
        thread_channel_id: thread_id(m),
    }
}

pub(super) fn project_message_detail(m: &Message) -> MessageDetail {
    MessageDetail {
        id: m.id.to_string(),
        user: user_ref(&m.author),
        content: m.content.clone(),
        attachments: collect_attachments(m),
        replied_to: replied_to(m),
        mentions: mentions(m),
        thread_channel_id: thread_id(m),
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
