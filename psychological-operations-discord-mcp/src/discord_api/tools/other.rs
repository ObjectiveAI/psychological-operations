//! Misc tools that are neither reads nor writes against Discord — they touch no
//! Discord API, just local/derived data, so they're quota-free (absent from
//! `ToolName`) like the queue tools.

use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsDiscordMcp;
use super::super::tool_error::{ToolError, finish};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct InviteLinkRequest {}

#[tool_router(router = other_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {
    #[tool(
        name = "invite_link",
        description = "Generate the bot's Discord invite URL. Send it to people so they can add \
                       you to their own server. The bot joins with no extra permissions (just \
                       the @everyone baseline). Quota-free."
    )]
    async fn invite_link(
        &self,
        Parameters(_req): Parameters<InviteLinkRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let client_id = self
                    .db
                    .discord_auth_get(&tag)
                    .await?
                    .and_then(|a| a.client_id)
                    .ok_or_else(|| {
                        ToolError::agent(format!(
                            "agent '{tag}' has no Discord client id — it isn't set up yet."
                        ))
                    })?;
                // permissions=0 (permissionless; the bot lands at the @everyone
                // baseline); scopes add the bot + slash commands. The scope
                // separator MUST be `+` — a `%20`-encoded space makes Discord
                // drop the `bot` scope (no add-to-server).
                let url = format!(
                    "https://discord.com/oauth2/authorize?client_id={client_id}\
                     &permissions=0&scope=bot+applications.commands"
                );
                Ok(CallToolResult::success(vec![Content::text(url)]))
            }
            .await,
        )
    }
}
