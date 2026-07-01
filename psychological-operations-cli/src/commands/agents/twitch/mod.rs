//! `agents twitch` subcommand surface.
//!
//! Twitch has no chat-history API, so the daemon holds a live IRC connection
//! per agent and buffers every message on the channels the agent has JOINed
//! into `twitch_messages` (which the `twitch` MCP's `list_messages` reads).
//! `channels add|remove|list` manages that per-agent JOIN set; a running daemon
//! picks up changes via the `twitch_channels` NOTIFY trigger.

use clap::Subcommand;

pub mod channels;

#[derive(Subcommand)]
pub enum Commands {
    /// Manage which Twitch channels the daemon JOINs (and buffers chat from)
    /// for an agent: `channels add|remove|list`.
    #[command(name = "channels")]
    Channels {
        #[command(subcommand)]
        command: channels::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Channels { command } => command.handle(ctx).await,
        }
    }
}
