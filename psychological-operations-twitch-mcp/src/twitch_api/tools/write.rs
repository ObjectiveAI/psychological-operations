//! Write tools — Twitch chat send through the Helix-backed client.
//!
//! `send_message` acts as the session's `tag`, is gated to `Mode::Full` (see
//! `FULL_ONLY_TOOLS` in `crate::mode`), and is metered against the write
//! budget. It goes through the SDK [`Client`]'s uncached send; the body runs
//! inside [`finish`] so failures classify the same way the read tools do.
//!
//! [`Client`]: psychological_operations_sdk::twitch::Client

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsTwitchMcp;
use super::super::model::SentMessage;
use super::super::tool_error::{ToolError, finish};
use super::read::normalize_channel;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageRequest {
    #[schemars(description = "The channel login to send to (a leading '#' is optional).")]
    pub channel: String,
    #[schemars(description = "The message text to send.")]
    pub content: String,
    #[schemars(description = "Optional: the id of a message in this channel to reply to. When \
                             set, the message is sent as a reply.")]
    pub reply_to_message_id: Option<String>,
}

#[tool_router(router = write_tools, vis = "pub")]
impl PsychologicalOperationsTwitchMcp {
    #[tool(
        name = "send_message",
        description = "Send a chat message to a channel. With reply_to_message_id, it's sent as \
                       a reply."
    )]
    async fn send_message(
        &self,
        Parameters(req): Parameters<SendMessageRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let tag = state.tag.clone();
        let max_len = state.max_message_length;
        finish(
            async move {
                check_message_length(&req.content, max_len)?;
                let channel = normalize_channel(&req.channel);
                let client = self.build_client();

                // Resolve the target channel's broadcaster id (its Twitch user
                // id). A missing channel is agent-actionable.
                let broadcaster = client
                    .get_user_by_login(&tag, &channel)
                    .await?
                    .ok_or_else(|| {
                        ToolError::agent(format!("no such Twitch channel: {channel}"))
                    })?;

                // Resolve the agent's OWN Twitch user id — the message sender.
                // Prefer the stored `twitch_auth.user_id`; fall back to a live
                // `validate` if the row didn't capture it.
                let sender_id = match self.db.twitch_auth_get(&tag).await?.and_then(|a| a.user_id) {
                    Some(id) => id,
                    None => client.validate(&tag).await?.user_id,
                };

                let sent = client
                    .send_message(
                        &tag,
                        &broadcaster.id,
                        &sender_id,
                        &req.content,
                        req.reply_to_message_id.as_deref(),
                    )
                    .await?;
                let result = SentMessage {
                    channel,
                    message_id: sent.message_id,
                };
                let body = serde_json::to_string(&result)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }
}

/// Reject message content that exceeds the session's `max_message_length`
/// (defaults to Twitch's 500 limit; see [`crate::twitch_api::session`]). Counts
/// Unicode scalar values. Surfaces as an agent-visible error result so the
/// model can shorten and retry. (Mirrors the Discord MCP's `check_message_length`.)
fn check_message_length(content: &str, max: usize) -> Result<(), ToolError> {
    let n = content.chars().count();
    if n > max {
        return Err(ToolError::agent(format!(
            "content is {n} characters, over the {max}-character limit — shorten it and try again."
        )));
    }
    Ok(())
}
