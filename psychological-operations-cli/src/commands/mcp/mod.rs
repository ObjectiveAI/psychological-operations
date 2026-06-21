//! `mcp` subcommand surface.
//!
//! `mcp x-api begin` runs the X-API MCP server in-process: this CLI
//! process becomes the MCP server. The server emits a single
//! `{"type":"mcp","url":"http://…"}` JSONL line on stdout — the
//! typed [`psychological_operations_sdk::cli::Output::Mcp`] (re-
//! exported from `objectiveai_sdk::cli::plugins::Output::Mcp`)
//! variant — when its listener binds. The objectiveai supervisor
//! parses that as an MCP-URL announcement and dials the URL
//! through the same path a manifest `mcp_servers` entry uses.
//! No supervisor, no child stderr, no `state.json`.
//!
//! Each server lives in its own submodule, nested under its name, because
//! objectiveai launches a plugin's MCP server as
//! `<plugin-exec> mcp <name> begin` (see `objectiveai-cli` conduit). The
//! `<name>` matches the plugin manifest's `mcp_servers[].name` — today the
//! only one is [`x_api`] (`x-api`).

use clap::Subcommand;

pub mod x_api;

#[derive(Subcommand)]
pub enum Commands {
    /// X-API MCP server commands. Nested under the server's name so the
    /// objectiveai host's `mcp <name> begin` plugin-launch convention
    /// resolves here (`<name>` = `x-api`).
    #[command(name = "x-api")]
    XApi {
        #[command(subcommand)]
        command: x_api::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::XApi { command } => command.handle(ctx).await,
        }
    }
}
