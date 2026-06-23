//! `agents invite discord` — print the bot's Discord server-invite URL.
//!
//! No browser, no network: read the agent's stored client id and format the
//! OAuth2 authorize link. Permissionless (permissions=0 — the bot joins at the
//! @everyone baseline) with the bot + applications.commands scopes.

use psychological_operations_sdk::cli::output::DiscordInvite;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::error::Error;

pub async fn run(name: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(name, ctx).await)
}

async fn run_inner(name: &str, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
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
    // `permissions=0` — no extra permissions; the bot lands at the @everyone
    // baseline. Scopes: `bot` to add the bot, `applications.commands` for slash
    // commands.
    let url = format!(
        "https://discord.com/oauth2/authorize?client_id={client_id}\
         &permissions=0&scope=bot%20applications.commands"
    );
    Ok(CliOutput::DiscordInvite(DiscordInvite { url }))
}
