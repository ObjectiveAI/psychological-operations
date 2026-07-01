//! `agents daemon` — manage the per-agent daemon configuration. Two platforms:
//! `daemon discord` (the Discord gateway daemon's hooks) and `daemon twitch`
//! (the Twitch IRC daemon's hooks).

use clap::Subcommand;

pub mod discord;
pub mod twitch;

#[derive(Subcommand)]
pub enum Commands {
    /// Discord gateway daemon config: `discord hooks {add,list,delete}`.
    #[command(name = "discord")]
    Discord {
        #[command(subcommand)]
        command: discord::Commands,
    },
    /// Twitch IRC daemon config: `twitch hooks {add,list,delete}`.
    #[command(name = "twitch")]
    Twitch {
        #[command(subcommand)]
        command: twitch::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Discord { command } => command.handle(ctx).await,
            Commands::Twitch { command } => command.handle(ctx).await,
        }
    }
}
