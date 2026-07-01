//! Read tools — Twitch reads.
//!
//! Each tool acts as the session's `tag` (from the `X-OBJECTIVEAI-ARGUMENTS`
//! header); mode-gating and the per-tag quota gate run centrally in `call_tool`
//! before dispatch. Bodies run inside [`finish`] so failures classify (see
//! [`super::super::tool_error`]).
//!
//! `whoami` is a live Helix/OAuth call; `list_channels` and `list_messages`
//! read ONLY the postgres buffer the daemon fills (they never call Twitch).

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsTwitchMcp;
use super::super::model::{MessageSummary, WhoAmI};
use super::super::projection::message_summary;
use super::super::tool_error::{ToolError, finish};

/// Max window size the paginated read tools accept.
const MAX_COUNT: u32 = 100;

/// Reject a `count` over [`MAX_COUNT`] with an agent-visible message.
fn check_count(count: u32) -> Result<(), ToolError> {
    if count > MAX_COUNT {
        return Err(ToolError::agent(format!(
            "count is {count}, over the {MAX_COUNT} max — request {MAX_COUNT} or fewer."
        )));
    }
    Ok(())
}

/// Normalize a Twitch channel reference to its canonical login: trim
/// surrounding whitespace, drop a leading `#`, and lowercase. (Twitch logins
/// are lowercase; chat commonly writes them as `#channel`.)
pub(super) fn normalize_channel(raw: &str) -> String {
    raw.trim().trim_start_matches('#').to_lowercase()
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoamiRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListChannelsRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListMessagesRequest {
    #[schemars(description = "The channel login to read messages from (a leading '#' is \
                             optional). One of the channels from list_channels.")]
    pub channel: String,
    #[schemars(description = "How many messages to return (after skipping `offset`; max 100). \
                             Newest first.")]
    pub count: u32,
    #[schemars(description = "How many messages to skip from the start (newest first).")]
    pub offset: u32,
}

#[tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsTwitchMcp {
    #[tool(
        name = "whoami",
        description = "Get the bot's own Twitch identity (the agent acts as this user)."
    )]
    async fn whoami(
        &self,
        Parameters(_req): Parameters<WhoamiRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let me = self.build_client().validate(&tag).await?;
                let who = WhoAmI {
                    user_id: me.user_id,
                    login: me.login,
                };
                let body = serde_json::to_string(&who)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_channels",
        description = "List the Twitch channels buffered for you (the ones the daemon is \
                       listening to on your behalf)."
    )]
    async fn list_channels(
        &self,
        Parameters(_req): Parameters<ListChannelsRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let channels = self.db.twitch_channels_list(&tag).await?;
                let body = serde_json::to_string(&channels)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_messages",
        description = "Read a buffered channel's chat messages, newest first. Reads only the \
                       daemon-filled buffer (never live Twitch)."
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
                let channel = normalize_channel(&req.channel);
                let messages = self
                    .db
                    .twitch_messages_list(&tag, &channel, req.count as i64, req.offset as i64)
                    .await?;
                let projected: Vec<MessageSummary> =
                    messages.iter().map(message_summary).collect();
                // Exact remaining count from the buffer (not a cursor): total in
                // the buffer minus what this window already covers.
                let total = self.db.twitch_messages_count(&tag, &channel).await? as usize;
                let remaining = total.saturating_sub(req.offset as usize + projected.len());
                let note = format!("{remaining} remaining");
                let body = serde_json::to_string(&projected)?;
                Ok(CallToolResult::success(vec![
                    Content::text(body),
                    Content::text(note),
                ]))
            }
            .await,
        )
    }
}
