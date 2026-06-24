//! Write tools — Discord mutations through the per-agent serenity client.
//!
//! Each tool acts as the session's `tag`, is gated to `Mode::Full` (see
//! `FULL_ONLY_TOOLS` in `super::super`), and is metered against the write
//! budget. All REST goes through the SDK [`Client`]'s write methods (uncached);
//! bodies run inside [`finish`] so failures classify the same way the read
//! tools do.
//!
//! [`Client`]: psychological_operations_sdk::discord::Client

use psychological_operations_sdk::discord::serenity;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serenity::all::{ChannelId, MessageId, ReactionType, UserId};

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
pub struct EditMessageRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The id of the bot's own message to edit.")]
    pub message_id: String,
    #[schemars(description = "The new message text (replaces the existing content).")]
    pub content: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteMessageRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The id of the bot's own message to delete.")]
    pub message_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateThreadRequest {
    #[schemars(description = "The parent channel (snowflake) to create the thread in.")]
    pub channel_id: String,
    #[schemars(description = "The thread's name.")]
    pub name: String,
    #[schemars(description = "Optional: a message id (any author) to start the thread from. \
                             Without it, a standalone public thread is created.")]
    pub message_id: Option<String>,
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
                check_message_length(&req.content)?;
                let channel = parse_channel(&req.channel_id)?;
                let reply_to = parse_opt_message(req.reply_to_message_id.as_deref())?;
                let msg = self
                    .build_client()
                    .send_message(&tag, channel, req.content, reply_to)
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
                check_message_length(&req.content)?;
                let user: UserId = req
                    .user_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid user id: {}", req.user_id)))?;
                let reply_to = parse_opt_message(req.reply_to_message_id.as_deref())?;
                let msg = self
                    .build_client()
                    .send_direct_message(&tag, user, req.content, reply_to)
                    .await?;
                let result = SentMessage {
                    channel_id: msg.channel_id.to_string(),
                    message_id: msg.id.to_string(),
                };
                let body = serde_json::to_string(&result)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "edit_message",
        description = "Edit one of the bot's own messages (replaces its content)."
    )]
    async fn edit_message(
        &self,
        Parameters(req): Parameters<EditMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_message_length(&req.content)?;
                let channel = parse_channel(&req.channel_id)?;
                let message = parse_message(&req.message_id)?;
                self.build_client()
                    .edit_message(&tag, channel, message, req.content)
                    .await?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({ "ok": true }).to_string(),
                )]))
            }
            .await,
        )
    }

    #[tool(
        name = "delete_message",
        description = "Delete one of the bot's own messages."
    )]
    async fn delete_message(
        &self,
        Parameters(req): Parameters<DeleteMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channel = parse_channel(&req.channel_id)?;
                let message = parse_message(&req.message_id)?;
                self.build_client()
                    .delete_message(&tag, channel, message)
                    .await?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({ "ok": true }).to_string(),
                )]))
            }
            .await,
        )
    }

    #[tool(
        name = "create_thread",
        description = "Create a thread in a channel. With message_id, it's started from that \
                       message (any author); without, a standalone public thread. Returns the \
                       thread's channel_id."
    )]
    async fn create_thread(
        &self,
        Parameters(req): Parameters<CreateThreadRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channel = parse_channel(&req.channel_id)?;
                let from = parse_opt_message(req.message_id.as_deref())?;
                let thread = self
                    .build_client()
                    .create_thread(&tag, channel, req.name, from)
                    .await?;
                let body = serde_json::json!({ "channel_id": thread.id.to_string() }).to_string();
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
                self.build_client()
                    .add_reaction(&tag, channel, message, rt)
                    .await?;
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
                self.build_client()
                    .remove_reaction(&tag, channel, message, rt)
                    .await?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({ "ok": true }).to_string(),
                )]))
            }
            .await,
        )
    }
}

/// Discord's standard message character limit. Content over this is rejected by
/// Discord; we reject proactively with an agent-visible message so the agent
/// shortens (or splits) its text instead of erroring later.
const MESSAGE_CHAR_LIMIT: usize = 2000;

/// Reject message content that exceeds the Discord character limit. Counts
/// Unicode scalar values. Surfaces as an agent-visible error result so the
/// model can shorten and retry. (Mirrors the X MCP's `check_tweet_length`.)
fn check_message_length(content: &str) -> Result<(), ToolError> {
    let n = content.chars().count();
    if n > MESSAGE_CHAR_LIMIT {
        return Err(ToolError::agent(format!(
            "content is {n} characters, over the {MESSAGE_CHAR_LIMIT}-character limit — shorten it and try again."
        )));
    }
    Ok(())
}

/// Parse a channel snowflake (agent error on bad input).
fn parse_channel(id: &str) -> Result<ChannelId, ToolError> {
    id.parse()
        .map_err(|_| ToolError::agent(format!("invalid channel id: {id}")))
}

/// Parse a message snowflake (agent error on bad input).
fn parse_message(id: &str) -> Result<MessageId, ToolError> {
    id.parse()
        .map_err(|_| ToolError::agent(format!("invalid message id: {id}")))
}

/// Parse an optional message snowflake (e.g. a reply target).
fn parse_opt_message(id: Option<&str>) -> Result<Option<MessageId>, ToolError> {
    id.map(parse_message).transpose()
}

/// Parse a [`ReactionRequest`]'s channel / message ids and emoji.
fn parse_reaction(req: &ReactionRequest) -> Result<(ChannelId, MessageId, ReactionType), ToolError> {
    let channel = parse_channel(&req.channel_id)?;
    let message = parse_message(&req.message_id)?;
    let rt: ReactionType = req
        .emoji
        .parse()
        .map_err(|_| ToolError::agent(format!("invalid emoji: {}", req.emoji)))?;
    Ok((channel, message, rt))
}
