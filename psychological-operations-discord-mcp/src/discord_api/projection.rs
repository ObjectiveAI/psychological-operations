//! Projection: serenity model types → the agent-facing shapes in
//! [`super::model`]. `author` is the global username (`User.name`); the reply
//! target is the referenced message id; `mentions` are the @-mentioned users'
//! global usernames.

use psychological_operations_sdk::discord::serenity;
use serenity::all::{GuildChannel, Message};

use super::model::{
    Attachment, AttachmentKind, AvailableReaction, ChannelInfo, MessageDetail, MessageSummary,
    ReactionSummary, RoleInfo, User,
};

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

/// The id of the message this one replies to, if it's a reply.
fn replied_to(m: &Message) -> Option<String> {
    m.message_reference
        .as_ref()
        .and_then(|r| r.message_id)
        .map(|id| id.to_string())
}

/// The reactions present on a message: each emoji's string form, count, and
/// whether the bot reacted.
fn reactions(m: &Message) -> Vec<ReactionSummary> {
    m.reactions
        .iter()
        .map(|r| ReactionSummary {
            emoji: r.reaction_type.to_string(),
            count: r.count,
            me: r.me,
        })
        .collect()
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
        thread_channel_id: thread_id(m),
        reactions: reactions(m),
        created_at: m.timestamp.to_string(),
    }
}

pub(super) fn available_reaction(e: &serenity::all::Emoji) -> AvailableReaction {
    AvailableReaction {
        name: e.name.clone(),
        id: e.id.to_string(),
        animated: e.animated,
    }
}

pub(super) fn channel_info(c: &GuildChannel) -> ChannelInfo {
    ChannelInfo {
        id: c.id.to_string(),
        name: c.name.clone(),
        kind: c.kind.name().to_string(),
    }
}

pub(super) fn role_info(r: &serenity::all::Role) -> RoleInfo {
    RoleInfo {
        id: r.id.to_string(),
        name: r.name.clone(),
        color: format!("#{}", r.colour.hex()),
        position: r.position,
        hoist: r.hoist,
        mentionable: r.mentionable,
        managed: r.managed,
    }
}
