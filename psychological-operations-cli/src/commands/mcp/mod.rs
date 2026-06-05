//! `mcp` subcommand surface.
//!
//! `mcp begin` runs the X-API MCP server in-process: this CLI
//! process becomes the MCP server. The MCP itself emits a single
//! `{"value":{"type":"mcp","url":"http://…"}}` JSONL line on stdout
//! when its listener binds; the host re-wraps it as a
//! `PluginOutput::Notification`. No supervisor, no child stderr,
//! no `state.json`.

use std::time::Duration;

use clap::Subcommand;
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the X-API MCP server in-process. Binds a random
    /// localhost port, emits one JSONL line with the URL, then
    /// serves until the process is killed. Per-session
    /// `(agent, mode)` are supplied by the client on connect via
    /// the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header (with
    /// `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the agent
    /// fallback) — this command takes neither.
    Begin {
        /// Cache budget in bytes (default 256 MiB).
        #[arg(long, default_value_t = 256 * 1024 * 1024)]
        cache_max_size: u64,
        /// Per-entry cache TTL in seconds (default 3600).
        #[arg(long, default_value_t = 3600)]
        cache_ttl: u64,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                Commands::Begin { cache_max_size, cache_ttl } => {
                    let config_base_dir = ctx.config.objectiveai_base_dir();
                    psychological_operations_x_api_mcp::run(
                        "127.0.0.1",
                        0,
                        config_base_dir,
                        cache_max_size,
                        Duration::from_secs(cache_ttl),
                    )
                    .await
                    .map_err(|e| Error::Other(format!("mcp run: {e}")))?;
                    // Unreachable under the happy path — `run` returns
                    // only on bind failure or after the listener stops
                    // accepting (which only happens when the process
                    // is being torn down).
                    Ok(Output::Ok)
                }
            }
        }.await;
        crate::output::emit_result(result)
    }
}
