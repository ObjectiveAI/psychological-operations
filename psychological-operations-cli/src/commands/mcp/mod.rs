//! `mcp` subcommand surface.
//!
//! The supervisor (probe + spawn + state.json), the content-hashed
//! extract, and the embed binary const all live in `crate::mcp`.
//! This file owns the clap surface and the dispatch.

use clap::Subcommand;

use crate::error::Error;

/// Tool-surface mode for the spawned X-API MCP. Mirrors the
/// x-api-mcp binary's own `Mode` enum so the CLI can validate the
/// flag at parse time; the spawn step renders this back to the
/// `"readonly"` / `"full"` strings the child binary expects.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Readonly,
    Full,
}

impl Mode {
    pub fn as_arg_str(self) -> &'static str {
        match self {
            Mode::Readonly => "readonly",
            Mode::Full => "full",
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Begin (or attach to) the per-agent X-API MCP. Idempotent —
    /// returns the existing URL if a live instance is already running
    /// for this (agent, mode) pair; otherwise spawns a fresh detached
    /// MCP on a random localhost port and returns its URL.
    Begin {
        /// Agent name. Falls back to the
        /// `OBJECTIVEAI_AGENT_ID` env-derived default in
        /// `Config` when absent; errors if neither is set.
        #[arg(long)]
        agent: Option<String>,
        /// Tool-surface mode. `readonly` exposes only read tools;
        /// `full` adds the mutating tools (post / reply / quote /
        /// like / retweet / bookmark).
        #[arg(long, value_enum, default_value_t = Mode::Readonly)]
        mode: Mode,
        /// Cache budget in bytes (default 256 MiB).
        #[arg(long, default_value_t = 256 * 1024 * 1024)]
        cache_max_size: u64,
        /// Per-entry cache TTL in seconds (default 3600).
        #[arg(long, default_value_t = 3600)]
        cache_ttl: u64,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
        match self {
            Commands::Begin { agent, mode, cache_max_size, cache_ttl } => {
                let agent = agent
                    .or_else(|| cfg.objectiveai_agent_id.clone())
                    .ok_or_else(|| Error::Other(
                        "agent required — pass --agent or set \
                         OBJECTIVEAI_AGENT_ID".into()))?;
                crate::mcp::begin::run(&agent, mode, cache_max_size, cache_ttl, cfg).await
            }
        }
    }
}
