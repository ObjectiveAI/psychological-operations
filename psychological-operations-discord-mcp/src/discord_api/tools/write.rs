//! Write tools — Discord mutations through the per-agent serenity client.
//!
//! Each tool acts as the session's `tag`, is gated to `Mode::Full` (see
//! `FULL_ONLY_TOOLS` in `super::super`), and is metered against the write
//! budget. Bodies run inside [`finish`] so failures classify the same way the
//! read tools do.

use psychological_operations_sdk::discord::serenity;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serenity::all::{Builder, ChannelId, CreateMessage, MessageId, ReactionType, UserId};

use super::super::PsychologicalOperationsDiscordMcp;
use super::super::model::SentMessage;
use super::super::tool_error::{ToolError, finish};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageRequest {
    #[schemars(description = "The channel (snowflake) to send to. A thread is a channel — pass \
                             its id here too.")]
    pub channel_id: String,
    #[schemars(description = "The message text to send.")]
    pub content: String,
    #[schemars(description = "Optional: the id of a message in this channel to reply to. When \
                             set, the message is sent as a reply.")]
    pub reply_to_message_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendDirectMessageRequest {
    #[schemars(description = "The user (snowflake) to DM. The bot opens (or reuses) a DM with \
                             them.")]
    pub user_id: String,
    #[schemars(description = "The message text to send.")]
    pub content: String,
    #[schemars(description = "Optional: the id of a message in the DM to reply to. When set, \
                             the message is sent as a reply.")]
    pub reply_to_message_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReactionRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The message's snowflake id.")]
    pub message_id: String,
    #[schemars(description = "The emoji to react with: a unicode emoji (e.g. 👍) or a custom \
                             emoji as name:id (see list_available_reactions).")]
    pub emoji: String,
}

/// Send `content` to `channel`, optionally as a reply to `reply_to`. Returns the
/// created message.
async fn send_to_channel(
    http: &serenity::http::Http,
    channel: ChannelId,
    content: String,
    reply_to: Option<&str>,
) -> Result<serenity::all::Message, ToolError> {
    let mut builder = CreateMessage::new().content(content);
    if let Some(mid) = reply_to {
        let message_id: MessageId = mid
            .parse()
            .map_err(|_| ToolError::agent(format!("invalid reply_to_message_id: {mid}")))?;
        // `(channel, message_id)` is a reply reference in this channel.
        builder = builder.reference_message((channel, message_id));
    }
    Ok(builder.execute(http, (channel, None)).await?)
}

#[tool_router(router = write_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {
    #[tool(
        name = "send_message",
        description = "Send a message to a channel. With reply_to_message_id, it's sent as a \
                       reply."
    )]
    async fn send_message(
        &self,
        Parameters(req): Parameters<SendMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channel: ChannelId = req
                    .channel_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid channel id: {}", req.channel_id)))?;
                let http = self.build_client().http(&tag).await?;
                let msg =
                    send_to_channel(&http, channel, req.content, req.reply_to_message_id.as_deref())
                        .await?;
                let result = SentMessage {
                    channel_id: channel.to_string(),
                    message_id: msg.id.to_string(),
                };
                let body = serde_json::to_string(&result)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "send_direct_message",
        description = "Send a direct message to a user (opens a DM). With reply_to_message_id, \
                       it's sent as a reply."
    )]
    async fn send_direct_message(
        &self,
        Parameters(req): Parameters<SendDirectMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let user: UserId = req
                    .user_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid user id: {}", req.user_id)))?;
                let http = self.build_client().http(&tag).await?;
                let dm = user.create_dm_channel(&http).await?;
                let channel = dm.id;
                let msg =
                    send_to_channel(&http, channel, req.content, req.reply_to_message_id.as_deref())
                        .await?;
                let result = SentMessage {
                    channel_id: channel.to_string(),
                    message_id: msg.id.to_string(),
                };
                let body = serde_json::to_string(&result)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "add_reaction", description = "Add the bot's reaction to a message.")]
    async fn add_reaction(
        &self,
        Parameters(req): Parameters<ReactionRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let (channel, message, rt) = parse_reaction(&req)?;
                let http = self.build_client().http(&tag).await?;
                http.create_reaction(channel, message, &rt).await?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({ "ok": true }).to_string(),
                )]))
            }
            .await,
        )
    }

    #[tool(
        name = "remove_reaction",
        description = "Remove the bot's own reaction from a message."
    )]
    async fn remove_reaction(
        &self,
        Parameters(req): Parameters<ReactionRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let (channel, message, rt) = parse_reaction(&req)?;
                let http = self.build_client().http(&tag).await?;
                http.delete_reaction_me(channel, message, &rt).await?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({ "ok": true }).to_string(),
                )]))
            }
            .await,
        )
    }
}

/// Parse a [`ReactionRequest`]'s channel / message ids and emoji.
fn parse_reaction(req: &ReactionRequest) -> Result<(ChannelId, MessageId, ReactionType), ToolError> {
    let channel: ChannelId = req
        .channel_id
        .parse()
        .map_err(|_| ToolError::agent(format!("invalid channel id: {}", req.channel_id)))?;
    let message: MessageId = req
        .message_id
        .parse()
        .map_err(|_| ToolError::agent(format!("invalid message id: {}", req.message_id)))?;
    let rt: ReactionType = req
        .emoji
        .parse()
        .map_err(|_| ToolError::agent(format!("invalid emoji: {}", req.emoji)))?;
    Ok((channel, message, rt))
}
