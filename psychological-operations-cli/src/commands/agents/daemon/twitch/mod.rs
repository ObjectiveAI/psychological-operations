//! `agents daemon twitch` — manage the Twitch IRC daemon's hooks for an agent.

use clap::Subcommand;

pub mod hooks;

#[derive(Subcommand)]
pub enum Commands {
    /// Manage the agent's daemon hooks: `hooks {add,list,delete}`.
    #[command(name = "hooks")]
    Hooks {
        #[command(subcommand)]
        command: hooks::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Hooks { command } => command.handle(ctx).await,
        }
    }
}
