//! `agents quota` — quota management for an agent.
//!
//! Currently one subcommand: `grant`, itself split per platform
//! (`grant x` / `grant discord`).

use clap::Subcommand;

pub mod grant;

#[derive(Subcommand)]
pub enum Commands {
    /// Grant a time-bounded additive quota boost: `grant x` (X-API) or
    /// `grant discord` (Discord).
    Grant {
        #[command(subcommand)]
        command: grant::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Grant { command } => command.handle(ctx).await,
        }
    }
}
