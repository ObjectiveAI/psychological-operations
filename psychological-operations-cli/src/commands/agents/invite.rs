//! `agents invite` — platform invite links.
//!
//! Parent command; currently one subcommand: `discord`, which prints the
//! Discord server-invite URL for an agent's bot. No browser, no network —
//! it just reads the stored client id and formats the OAuth2 authorize link.

use clap::Subcommand;
use psychological_operations_sdk::cli::output::DiscordInvite;
use psychological_operations_sdk::cli::Output as CliOutput;

use super::agent_ref::AgentRef;
use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Print the Discord server-invite URL for this agent's bot. Requires
    /// `agents login discord` to have stored the bot's client id. The link
    /// grants Administrator (full permissions) with the bot +
    /// applications.commands scopes.
    #[command(name = "discord")]
    Discord {
        #[command(flatten)]
        agent: AgentRef,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Discord { agent } => {
                let name = agent.resolve_raw(&ctx.config);
                crate::output::emit_result(discord(&name, ctx).await)
            }
        }
    }
}

async fn discord(name: &str, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    let client_id = ctx
        .db
        .discord_auth_get(name)
        .await?
        .and_then(|a| a.client_id)
        .ok_or_else(|| {
            Error::Other(format!(
                "agent '{name}' has no Discord bot client id — run `agents login discord` first"
            ))
        })?;
    // `permissions=8` is Administrator — full permissions across the board.
    // Scopes: `bot` to add the bot, `applications.commands` for slash commands.
    let url = format!(
        "https://discord.com/oauth2/authorize?client_id={client_id}\
         &permissions=8&scope=bot%20applications.commands"
    );
    Ok(CliOutput::DiscordInvite(DiscordInvite { url }))
}
