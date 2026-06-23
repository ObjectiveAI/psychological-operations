//! Read tools — Discord reads through the per-agent serenity client.
//!
//! Each tool acts as the session's `tag` (from the `X-OBJECTIVEAI-ARGUMENTS`
//! header): it builds the shared [`discord::Client`] and resolves that agent's
//! bot `Http`; mode-gating and the per-tag quota gate run centrally in
//! `call_tool` before dispatch.
//!
//! Each body runs inside [`finish`] so failures classify (see
//! [`super::super::tool_error`]): a missing bot token / db error surfaces as a
//! protocol error; the authorized request's own rejection (missing perms,
//! not found) and bad agent inputs surface as `is_error` tool results.
//!
//! Discovery → read → drill in: `list_servers` → `list_channels` →
//! `list_messages` (slim summaries) → `get_message` (full) / `open_attachment`.

use base64::Engine;
use objectiveai_sdk::agent::completions::message::{File, ImageUrl, RichContentPart, VideoUrl};
use objectiveai_sdk::mcp::tool::ContentBlock;
use psychological_operations_sdk::discord::serenity;
use rmcp::model::{CallToolResult, Content, Extensions, RawContent};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serenity::all::{ChannelId, GuildId, MessageId, MessagePagination};

use super::super::PsychologicalOperationsDiscordMcp;
use super::super::model::{AttachmentKind, ChannelInfo, MessageSummary, ServerInfo};
use super::super::projection::{
    attachment_kind, channel_info, project_message_detail, project_message_summary,
};
use super::super::tool_error::{ToolError, finish};

/// Max window size the paginated read tools (and the queue tools) accept.
const MAX_COUNT: u32 = 100;

/// Discord's max page size for `GET /channels/{id}/messages`.
const MESSAGES_PAGE: usize = 100;

/// Reject a `count` over [`MAX_COUNT`] with an agent-visible message.
pub(super) fn check_count(count: u32) -> Result<(), ToolError> {
    if count > MAX_COUNT {
        return Err(ToolError::agent(format!(
            "count is {count}, over the {MAX_COUNT} max — request {MAX_COUNT} or fewer."
        )));
    }
    Ok(())
}

/// A short "N remaining" note appended to a windowed list result.
pub(super) fn remaining_note(
    total_fetched: usize,
    offset: usize,
    count: usize,
    has_more: bool,
) -> String {
    let remaining = total_fetched.saturating_sub(offset + count);
    format!("{}{remaining} remaining", if has_more { "over " } else { "" })
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListServersRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListChannelsRequest {
    #[schemars(description = "The server (guild) snowflake id to list channels for.")]
    pub server_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListMessagesRequest {
    #[schemars(description = "The channel (snowflake) to read messages from. A thread is a \
                             channel — pass its id here too.")]
    pub channel_id: String,
    #[schemars(description = "How many messages to return (after skipping `offset`; max 100). \
                             Newest first.")]
    pub count: u32,
    #[schemars(description = "How many messages to skip from the start (newest first).")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetMessageRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The message's snowflake id.")]
    pub message_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenAttachmentRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The message's snowflake id.")]
    pub message_id: String,
    #[schemars(description = "Attachment URL as returned in get_message's attachments[].url.")]
    pub url: String,
}

fn parse_channel(id: &str) -> Result<ChannelId, ToolError> {
    id.parse()
        .map_err(|_| ToolError::agent(format!("invalid channel id: {id}")))
}

fn parse_message(id: &str) -> Result<MessageId, ToolError> {
    id.parse()
        .map_err(|_| ToolError::agent(format!("invalid message id: {id}")))
}

/// Convert an objectiveai [`RichContentPart`] into an rmcp [`Content`] block
/// via the SDK's `RichContentPart -> ContentBlock` converter, so attachments
/// are formatted the way the objectiveai system expects (and round-trip back to
/// the right rich type). The objectiveai `ContentBlock` and rmcp `Content`
/// share the MCP wire shape, so the bridge is a serde round-trip.
fn rich_content(part: RichContentPart) -> Result<Content, ToolError> {
    let block = ContentBlock::from(part);
    let value = serde_json::to_value(&block)?;
    let raw: RawContent = serde_json::from_value(value)?;
    Ok(Content {
        raw,
        annotations: None,
    })
}

#[tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {
    #[tool(
        name = "list_servers",
        description = "List the Discord servers (guilds) the bot is in."
    )]
    async fn list_servers(
        &self,
        Parameters(_req): Parameters<ListServersRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client().http(&tag).await?;
                let guilds = http.get_guilds(None, None).await?;
                let servers: Vec<ServerInfo> = guilds
                    .iter()
                    .map(|g| ServerInfo {
                        id: g.id.to_string(),
                        name: g.name.clone(),
                    })
                    .collect();
                let body = serde_json::to_string(&servers)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_channels",
        description = "List the channels in a server (guild), each with its type."
    )]
    async fn list_channels(
        &self,
        Parameters(req): Parameters<ListChannelsRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let guild: GuildId = req
                    .server_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid server id: {}", req.server_id)))?;
                let http = self.build_client().http(&tag).await?;
                let channels = http.get_channels(guild).await?;
                let infos: Vec<ChannelInfo> = channels.iter().map(channel_info).collect();
                let body = serde_json::to_string(&infos)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_messages",
        description = "Read a channel's messages, newest first. Returns slim summaries — \
                       open one with get_message for its full content."
    )]
    async fn list_messages(
        &self,
        Parameters(req): Parameters<ListMessagesRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let channel = parse_channel(&req.channel_id)?;
                let http = self.build_client().http(&tag).await?;

                let need = req.offset as usize + req.count as usize;
                // Page the history newest-first via `Before(cursor)` until we
                // have `need` messages or the channel runs out.
                let mut collected: Vec<serenity::all::Message> = Vec::new();
                let mut before: Option<MessageId> = None;
                let mut exhausted = false;
                while collected.len() < need {
                    let want = (need - collected.len()).min(MESSAGES_PAGE) as u8;
                    let target = before.map(MessagePagination::Before);
                    let batch = http.get_messages(channel, target, Some(want)).await?;
                    let got = batch.len();
                    if got == 0 {
                        exhausted = true;
                        break;
                    }
                    // Returned newest-first; the last element is the oldest —
                    // the cursor for the next (older) page.
                    before = batch.last().map(|m| m.id);
                    collected.extend(batch);
                    if (got as u8) < want {
                        exhausted = true;
                        break;
                    }
                }

                let projected: Vec<MessageSummary> =
                    collected.iter().map(project_message_summary).collect();
                let note = remaining_note(
                    projected.len(),
                    req.offset as usize,
                    req.count as usize,
                    !exhausted,
                );
                let window: Vec<MessageSummary> = projected
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .collect();
                let body = serde_json::to_string(&window)?;
                Ok(CallToolResult::success(vec![
                    Content::text(body),
                    Content::text(note),
                ]))
            }
            .await,
        )
    }

    #[tool(name = "get_message", description = "Fetch a Discord message in full.")]
    async fn get_message(
        &self,
        Parameters(req): Parameters<GetMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channel = parse_channel(&req.channel_id)?;
                let message = parse_message(&req.message_id)?;
                let http = self.build_client().http(&tag).await?;
                let m = http.get_message(channel, message).await?;
                let detail = project_message_detail(&m);
                let body = serde_json::to_string(&detail)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "open_attachment", description = "Fetch a message attachment's bytes.")]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channel = parse_channel(&req.channel_id)?;
                let message = parse_message(&req.message_id)?;
                let http = self.build_client().http(&tag).await?;
                let m = http.get_message(channel, message).await?;
                let att = m
                    .attachments
                    .iter()
                    .find(|a| a.url == req.url)
                    .ok_or_else(|| {
                        ToolError::agent(format!(
                            "attachment URL not on message {}: {}",
                            req.message_id, req.url,
                        ))
                    })?;
                let kind = attachment_kind(att.content_type.as_deref());
                let mime = att
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                let filename = att.filename.clone();
                let bytes = att.download().await?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                // Build the objectiveai RichContentPart, then convert to an MCP
                // content block via the SDK's converter so the objectiveai
                // system formats/round-trips it correctly.
                let part = match kind {
                    AttachmentKind::Image => RichContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: format!("data:{mime};base64,{b64}"),
                            detail: None,
                        },
                    },
                    AttachmentKind::Video => RichContentPart::VideoUrl {
                        video_url: VideoUrl {
                            url: format!("data:{mime};base64,{b64}"),
                        },
                    },
                    AttachmentKind::File => RichContentPart::File {
                        file: File {
                            file_data: Some(b64),
                            file_id: None,
                            filename: Some(filename),
                            file_url: None,
                        },
                    },
                };
                Ok(CallToolResult::success(vec![rich_content(part)?]))
            }
            .await,
        )
    }
}
