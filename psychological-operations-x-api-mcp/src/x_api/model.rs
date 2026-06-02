//! Agent-facing data shapes — the *only* custom Tweet projection in
//! the workspace. Everything else flows directly through the SDK's
//! codegen'd `x::types::*`.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AttachmentKind {
    Photo,
    Video,
    AnimatedGif,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Attachment {
    pub kind: AttachmentKind,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Tweet {
    pub id: String,
    pub handle: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quoted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retweeted: Option<String>,
    pub reply_count: i32,
}

#[derive(Debug, Clone)]
pub(super) struct FetchedAttachment {
    pub kind: AttachmentKind,
    pub mime: String,
    pub bytes: Vec<u8>,
}
