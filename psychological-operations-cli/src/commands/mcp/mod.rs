//! `mcp` subcommand surface.
//!
//! `mcp begin` runs the X-API MCP server in-process: this CLI
//! process becomes the MCP server. The server emits a single
//! `{"type":"mcp","url":"http://…"}` JSONL line on stdout — the
//! typed [`psychological_operations_sdk::cli::Output::Mcp`] (re-
//! exported from `objectiveai_sdk::cli::plugins::Output::Mcp`)
//! variant — when its listener binds. The objectiveai supervisor
//! parses that as an MCP-URL announcement and dials the URL
//! through the same path a manifest `mcp_servers` entry uses.
//! No supervisor, no child stderr, no `state.json`.

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
    /// fallback) — this command takes neither. Quota is now
    /// per-account, per-tool-call, configured via `agents quota`
    /// and stored in the db — no quota knobs here.
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
                    let state_dir = ctx.config.state_dir();
                    // Share the CLI's existing PluginExecutor. Every
                    // field is `Arc`-backed (including the id counter),
                    // so the clone is a logical handle to the same
                    // executor — pending map, stdout lock, liveness
                    // flag, and id sequence are all shared. The X-API
                    // server doesn't actually invoke it, but if it
                    // ever does the calls land on the same demuxer.
                    psychological_operations_x_api_mcp::run(
                        "127.0.0.1",
                        0,
                        state_dir,
                        ctx.db.clone(),
                        cache_max_size,
                        Duration::from_secs(cache_ttl),
                        (*ctx.executor).clone(),
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
