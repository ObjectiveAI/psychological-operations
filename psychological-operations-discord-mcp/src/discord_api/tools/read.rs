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
use rmcp::model::{CallToolResult, Content, Extensions, RawAudioContent, RawContent};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serenity::all::{
    ChannelId, GuildId, MessageId, MessagePagination, ReactionType, RoleId, UserId,
};

use super::super::PsychologicalOperationsDiscordMcp;
use super::super::model::{
    AttachmentKind, AvailableReaction, ChannelInfo, MessageSummary, ServerInfo, User, UserProfile,
};
use super::super::projection::{
    attachment_kind, available_reaction, channel_info, project_message_detail,
    project_message_summary, role_info, user_ref,
};
use super::super::tool_error::{ToolError, finish};

/// Max window size the paginated read tools (and the queue tools) accept.
const MAX_COUNT: u32 = 100;

/// Discord's max page size for `GET /channels/{id}/messages`.
const MESSAGES_PAGE: usize = 100;

/// Discord's max page size for `GET /guilds/{id}/members`.
const MEMBERS_PAGE: u64 = 1000;

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
pub struct WhoamiRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListChannelsRequest {
    #[schemars(description = "The server (guild) snowflake id to list channels for.")]
    pub server_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListUsersRequest {
    #[schemars(description = "The server (guild) snowflake id to list members of.")]
    pub server_id: String,
    #[schemars(description = "How many users to return (after skipping `offset`; max 100).")]
    pub count: u32,
    #[schemars(description = "How many users to skip from the start.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRoleRequest {
    #[schemars(description = "The server (guild) snowflake id.")]
    pub server_id: String,
    #[schemars(description = "The role's snowflake id.")]
    pub role_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListRoleMembersRequest {
    #[schemars(description = "The server (guild) snowflake id.")]
    pub server_id: String,
    #[schemars(description = "The role's snowflake id.")]
    pub role_id: String,
    #[schemars(description = "How many users to return (after skipping `offset`; max 100).")]
    pub count: u32,
    #[schemars(description = "How many users to skip from the start.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetUserRequest {
    #[schemars(description = "The user's snowflake id.")]
    pub user_id: String,
    #[schemars(description = "Optional server (guild) id. When given, `nickname` is the user's \
                             per-server nickname (falling back to their username if unset).")]
    pub server_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetProfilePictureRequest {
    #[schemars(description = "The user's snowflake id.")]
    pub user_id: String,
    #[schemars(description = "Optional server (guild) id. When given, returns the user's \
                             per-server avatar if they set one.")]
    pub server_id: Option<String>,
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
pub struct ListAvailableReactionsRequest {
    #[schemars(description = "Optional server (guild) id. Without it, lists the bot's globally \
                             usable emojis; with it, that server's custom emojis only.")]
    pub server_id: Option<String>,
    #[schemars(description = "How many emojis to return (after skipping `offset`; max 100).")]
    pub count: u32,
    #[schemars(description = "How many emojis to skip from the start.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetMessageReactionsByUserRequest {
    #[schemars(description = "The channel (snowflake) the message is in.")]
    pub channel_id: String,
    #[schemars(description = "The message's snowflake id.")]
    pub message_id: String,
    #[schemars(description = "The emoji to list reactors for: a unicode emoji (e.g. 👍) or a \
                             custom emoji as name:id.")]
    pub emoji: String,
    #[schemars(description = "How many users to return (after skipping `offset`; max 100).")]
    pub count: u32,
    #[schemars(description = "How many users to skip from the start.")]
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
/// the right rich type). The `ContentBlock` carriers map directly onto rmcp's
/// `RawContent` (the converter only ever yields Text / Image / Audio).
fn rich_content(part: RichContentPart) -> Result<Content, ToolError> {
    match ContentBlock::from(part) {
        ContentBlock::Text(t) => Ok(Content::text(t.text)),
        ContentBlock::Image(i) => Ok(Content::image(i.data, i.mime_type)),
        ContentBlock::Audio(a) => Ok(Content {
            raw: RawContent::Audio(RawAudioContent {
                data: a.data,
                mime_type: a.mime_type,
            }),
            annotations: None,
        }),
        ContentBlock::ResourceLink(_) | ContentBlock::EmbeddedResource(_) => Err(
            ToolError::System(ErrorData::internal_error(
                "unexpected resource content block from attachment".to_string(),
                None,
            )),
        ),
    }
}

#[tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {
    #[tool(
        name = "list_servers",
        description = "List the Discord servers (guilds) you're in."
    )]
    async fn list_servers(
        &self,
        Parameters(_req): Parameters<ListServersRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let guilds = self.build_client().get_guilds(&tag).await?;
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
        name = "whoami",
        description = "Get the bot's own Discord identity (the agent acts as this user)."
    )]
    async fn whoami(
        &self,
        Parameters(_req): Parameters<WhoamiRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let me = self.build_client().get_current_user(&tag).await?;
                let body = serde_json::to_string(&user_ref(&me))?;
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
                let channels = self.build_client().get_channels(&tag, guild).await?;
                let infos: Vec<ChannelInfo> = channels.iter().map(channel_info).collect();
                let body = serde_json::to_string(&infos)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_users",
        description = "List a server's members by global username."
    )]
    async fn list_users(
        &self,
        Parameters(req): Parameters<ListUsersRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let guild: GuildId = req
                    .server_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid server id: {}", req.server_id)))?;
                let http = self.build_client().http(&tag).await?;

                let need = req.offset as usize + req.count as usize;
                // Page members via the `after` (user-id) cursor until we have
                // `need` of them or the guild runs out.
                let mut users: Vec<User> = Vec::new();
                let mut after: Option<u64> = None;
                let mut exhausted = false;
                while users.len() < need {
                    let page = http.get_guild_members(guild, Some(MEMBERS_PAGE), after).await?;
                    if page.is_empty() {
                        exhausted = true;
                        break;
                    }
                    after = page.last().map(|m| m.user.id.get());
                    users.extend(page.iter().map(|m| user_ref(&m.user)));
                    if (page.len() as u64) < MEMBERS_PAGE {
                        exhausted = true;
                        break;
                    }
                }

                let note = remaining_note(
                    users.len(),
                    req.offset as usize,
                    req.count as usize,
                    !exhausted,
                );
                let window: Vec<User> = users
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

    #[tool(name = "get_role", description = "Get a role's info in a server.")]
    async fn get_role(
        &self,
        Parameters(req): Parameters<GetRoleRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let guild: GuildId = req
                    .server_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid server id: {}", req.server_id)))?;
                let role_id: RoleId = req
                    .role_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid role id: {}", req.role_id)))?;
                let http = self.build_client().http(&tag).await?;
                let role = http.get_guild_role(guild, role_id).await?;
                let body = serde_json::to_string(&role_info(&role))?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_role_members",
        description = "List the users who have a given role in a server."
    )]
    async fn list_role_members(
        &self,
        Parameters(req): Parameters<ListRoleMembersRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let guild: GuildId = req
                    .server_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid server id: {}", req.server_id)))?;
                let role_id: RoleId = req
                    .role_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid role id: {}", req.role_id)))?;
                let http = self.build_client().http(&tag).await?;

                let need = req.offset as usize + req.count as usize;
                // No "members with role X" endpoint — page all members and keep
                // the ones that carry the role, until we have `need` or run out.
                let mut matched: Vec<User> = Vec::new();
                let mut after: Option<u64> = None;
                let mut exhausted = false;
                while matched.len() < need {
                    let page = http.get_guild_members(guild, Some(MEMBERS_PAGE), after).await?;
                    if page.is_empty() {
                        exhausted = true;
                        break;
                    }
                    after = page.last().map(|m| m.user.id.get());
                    let full_page = (page.len() as u64) == MEMBERS_PAGE;
                    for m in &page {
                        if m.roles.contains(&role_id) {
                            matched.push(user_ref(&m.user));
                        }
                    }
                    if !full_page {
                        exhausted = true;
                        break;
                    }
                }

                let note = remaining_note(
                    matched.len(),
                    req.offset as usize,
                    req.count as usize,
                    !exhausted,
                );
                let window: Vec<User> = matched
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

    #[tool(
        name = "get_user",
        description = "Look up a user by id. With a server_id, `nickname` is their per-server \
                       nickname (else their username)."
    )]
    async fn get_user(
        &self,
        Parameters(req): Parameters<GetUserRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let uid: UserId = req
                    .user_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid user id: {}", req.user_id)))?;
                let http = self.build_client().http(&tag).await?;
                let profile = match req.server_id {
                    Some(sid) => {
                        let guild: GuildId = sid.parse().map_err(|_| {
                            ToolError::agent(format!("invalid server id: {sid}"))
                        })?;
                        let member = http.get_member(guild, uid).await?;
                        let user = user_ref(&member.user);
                        let nickname = member.nick.clone().unwrap_or_else(|| user.username.clone());
                        UserProfile {
                            user,
                            nickname,
                            bot: member.user.bot,
                        }
                    }
                    None => {
                        let u = http.get_user(uid).await?;
                        let nickname = u.name.clone();
                        UserProfile {
                            user: user_ref(&u),
                            nickname,
                            bot: u.bot,
                        }
                    }
                };
                let body = serde_json::to_string(&profile)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "get_profile_picture",
        description = "Get a user's avatar URL by id. With a server_id, prefers their \
                       per-server avatar."
    )]
    async fn get_profile_picture(
        &self,
        Parameters(req): Parameters<GetProfilePictureRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let uid: UserId = req
                    .user_id
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid user id: {}", req.user_id)))?;
                let http = self.build_client().http(&tag).await?;
                let url = match req.server_id {
                    Some(sid) => {
                        let guild: GuildId = sid.parse().map_err(|_| {
                            ToolError::agent(format!("invalid server id: {sid}"))
                        })?;
                        http.get_member(guild, uid).await?.face()
                    }
                    None => http.get_user(uid).await?.face(),
                };
                Ok(CallToolResult::success(vec![Content::text(url)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_messages",
        description = "Read a channel's messages, newest first."
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

    #[tool(
        name = "list_available_reactions",
        description = "List custom emojis the bot can react with. Without server_id: the bot's \
                       globally-usable emojis. With server_id: that server's custom emojis only. \
                       React with one by its name:id."
    )]
    async fn list_available_reactions(
        &self,
        Parameters(req): Parameters<ListAvailableReactionsRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let http = self.build_client().http(&tag).await?;
                let emojis = match req.server_id {
                    Some(sid) => {
                        let guild: GuildId = sid.parse().map_err(|_| {
                            ToolError::agent(format!("invalid server id: {sid}"))
                        })?;
                        http.get_emojis(guild).await?
                    }
                    None => http.get_application_emojis().await?,
                };
                let all: Vec<AvailableReaction> = emojis.iter().map(available_reaction).collect();
                // Full list in hand — `remaining` is exact (no cursor).
                let note =
                    remaining_note(all.len(), req.offset as usize, req.count as usize, false);
                let window: Vec<AvailableReaction> = all
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

    #[tool(
        name = "get_message_reactions_by_user",
        description = "List the users who reacted to a message with a given emoji."
    )]
    async fn get_message_reactions_by_user(
        &self,
        Parameters(req): Parameters<GetMessageReactionsByUserRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                check_count(req.count)?;
                let channel = parse_channel(&req.channel_id)?;
                let message = parse_message(&req.message_id)?;
                let rt: ReactionType = req
                    .emoji
                    .parse()
                    .map_err(|_| ToolError::agent(format!("invalid emoji: {}", req.emoji)))?;
                let http = self.build_client().http(&tag).await?;

                let need = req.offset as usize + req.count as usize;
                let mut users: Vec<User> = Vec::new();
                let mut after: Option<u64> = None;
                let mut exhausted = false;
                while users.len() < need {
                    let want = (need - users.len()).min(100) as u8;
                    let page = http
                        .get_reaction_users(channel, message, &rt, want, after)
                        .await?;
                    if page.is_empty() {
                        exhausted = true;
                        break;
                    }
                    after = page.last().map(|u| u.id.get());
                    users.extend(page.iter().map(user_ref));
                    if (page.len() as u8) < want {
                        exhausted = true;
                        break;
                    }
                }

                let note = remaining_note(
                    users.len(),
                    req.offset as usize,
                    req.count as usize,
                    !exhausted,
                );
                let window: Vec<User> = users
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

    #[tool(name = "open_attachment", description = "Fetch a message attachment.")]
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
