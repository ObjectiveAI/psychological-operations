//! `agents invite` — platform invite links.
//!
//! Parent command nesting one subcommand per platform; currently just
//! `discord`, which prints the Discord server-invite URL for an agent's bot.
//! Mirrors the `login` module's shape so new platforms slot in the same way.

use clap::Subcommand;

pub mod discord;

#[derive(Subcommand)]
pub enum Commands {
    /// Print the Discord server-invite URL for this agent's bot. Requires
    /// `agents login discord` to have stored the bot's client id. The link
    /// grants Administrator (full permissions) with the bot +
    /// applications.commands scopes.
    #[command(name = "discord")]
    Discord {
        /// Agent tag, used verbatim as the name.
        #[arg(long)]
        agent_tag: String,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Discord { agent_tag } => discord::run(&agent_tag, ctx).await,
        }
    }
}
