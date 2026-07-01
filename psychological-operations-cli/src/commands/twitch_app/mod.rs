//! `twitch-app` subcommand surface (the Rust module stays `twitch_app`;
//! clap renames the CLI command to the kebab-case `twitch-app`).

use clap::Subcommand;

pub mod setup;

#[derive(Subcommand)]
pub enum Commands {
    /// Set up the master Twitch application credentials. Spawns the embedded
    /// browser scoped to `twitch-app/` on the Twitch dev console; the overlay
    /// guides the operator to create/register the app and scrapes its
    /// `client_id` + `client_secret`, which fund every agent's OAuth.
    ///
    /// `--dangerously-reset` clears the stored app credentials first.
    #[command(name = "setup")]
    Setup {
        /// Clear the stored Twitch app credentials before launching.
        #[arg(long)]
        dangerously_reset: bool,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Setup { dangerously_reset } => setup::run(dangerously_reset, ctx).await,
        }
    }
}
