//! `agents quota grant` — time-bounded additive quota boosts, per platform.
//!
//! `grant x` boosts the X-API MCP's read/write budget; `grant discord` boosts
//! the (separate) Discord MCP budget. Both share the same arg shape and the
//! read/write [`Direction`].

use clap::{Subcommand, ValueEnum};

pub mod discord;
pub mod x;

#[derive(Subcommand)]
pub enum Commands {
    /// Grant an agent an X-API quota boost. While in effect it raises that
    /// direction's available quota by `--quantity`. Grants stack.
    #[command(name = "x")]
    X {
        /// Which budget to boost.
        #[arg(long, value_enum)]
        mode: Direction,
        /// Agent tag to grant the boost to.
        #[arg(long)]
        agent_tag: String,
        /// Flat amount to add to the available quota while in effect.
        #[arg(long)]
        quantity: u64,
        /// How long the grant stays in effect (humantime, e.g. "1h", "30m").
        #[arg(long)]
        duration: String,
    },
    /// Grant an agent a Discord quota boost. Same semantics as `grant x` but
    /// against the Discord budget.
    #[command(name = "discord")]
    Discord {
        /// Which budget to boost.
        #[arg(long, value_enum)]
        mode: Direction,
        /// Agent tag to grant the boost to.
        #[arg(long)]
        agent_tag: String,
        /// Flat amount to add to the available quota while in effect.
        #[arg(long)]
        quantity: u64,
        /// How long the grant stays in effect (humantime, e.g. "1h", "30m").
        #[arg(long)]
        duration: String,
    },
}

/// The quota budget a grant targets. Serializes to the `"read"` / `"write"`
/// strings the DB + MCP agree on.
#[derive(Clone, Copy, ValueEnum)]
pub enum Direction {
    Read,
    Write,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Read => "read",
            Direction::Write => "write",
        }
    }
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::X {
                mode,
                agent_tag,
                quantity,
                duration,
            } => x::run(ctx, mode, &agent_tag, quantity, &duration).await,
            Commands::Discord {
                mode,
                agent_tag,
                quantity,
                duration,
            } => discord::run(ctx, mode, &agent_tag, quantity, &duration).await,
        }
    }
}
