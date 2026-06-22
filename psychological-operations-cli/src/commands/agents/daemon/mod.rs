//! `agents daemon` — manage the per-agent daemon configuration. Currently one
//! platform: `daemon discord` (the Discord gateway daemon's hooks).

use clap::Subcommand;

pub mod discord;

#[derive(Subcommand)]
pub enum Commands {
    /// Discord gateway daemon config: `discord hooks {add,list,delete}`.
    #[command(name = "discord")]
    Discord {
        #[command(subcommand)]
        command: discord::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Discord { command } => command.handle(ctx).await,
        }
    }
}
