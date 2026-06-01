//! `mcp` subcommand surface.
//!
//! Today's only subcommand is `begin` — a probe-or-spawn supervisor
//! that ensures one `psychological-operations-x-api-mcp` instance per
//! agent is running on the local machine and returns its URL. The
//! embedded X-API MCP binary is content-hashed and extracted into a
//! per-binary temp dir; per-agent state lives in a separate
//! stable-named temp dir so re-invocations can re-attach.
//!
//! See `embed.rs` for the build-time `include_bytes!` + content-hashed
//! extract; see `begin.rs` for the supervisor logic.

use clap::Subcommand;

use crate::error::Error;

pub mod begin;
pub mod embed;

#[derive(Subcommand)]
pub enum Commands {
    /// Begin (or attach to) the per-agent X-API MCP. Idempotent —
    /// returns the existing URL if a live instance is already running
    /// for this agent; otherwise spawns a fresh detached MCP on a
    /// random localhost port and returns its URL.
    Begin {
        /// Agent name. Defaults to `OBJECTIVEAI_AGENT_ID_BASE` env;
        /// errors if neither is set.
        #[arg(long)]
        agent: Option<String>,
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
            Commands::Begin { agent, cache_max_size, cache_ttl } => {
                begin::run(agent, cache_max_size, cache_ttl, cfg).await
            }
        }
    }
}
