//! `agents quota` — quota management for an agent.
//!
//! `grant` records a time-bounded additive boost to an agent's read or
//! write available quota. While the grant is in effect the x-api MCP
//! server adds its amount to that direction's limit; grants stack.

use clap::{Subcommand, ValueEnum};
use psychological_operations_db::unix_now;
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Grant an agent a time-bounded additive quota boost. While it is in
    /// effect it raises that direction's available quota by `--quantity`.
    /// Grants stack (active grants sum).
    Grant {
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
    fn as_str(self) -> &'static str {
        match self {
            Direction::Read => "read",
            Direction::Write => "write",
        }
    }
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Grant {
                mode,
                agent_tag,
                quantity,
                duration,
            } => {
                crate::output::emit_result(grant(ctx, mode, &agent_tag, quantity, &duration).await)
            }
        }
    }
}

async fn grant(
    ctx: &crate::context::Context,
    mode: Direction,
    agent_tag: &str,
    quantity: u64,
    duration: &str,
) -> Result<Output, Error> {
    let secs = humantime::parse_duration(duration)
        .map_err(|e| Error::Other(format!("invalid duration: {e}")))?
        .as_secs() as i64;
    let granted_at = unix_now();
    let expires_at = granted_at + secs;
    ctx.db
        .grant_quota(
            agent_tag,
            mode.as_str(),
            quantity as i64,
            granted_at,
            expires_at,
        )
        .await
        .map_err(|e| Error::Other(format!("quota grant: {e}")))?;
    Ok(Output::Ok)
}
