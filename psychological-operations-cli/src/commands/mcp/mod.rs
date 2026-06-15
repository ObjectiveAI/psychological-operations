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
//! The server is nested under its name (`x-api`) because objectiveai
//! launches a plugin's MCP server as `<plugin-exec> mcp <name> begin`
//! (see `objectiveai-cli` conduit). The `<name>` matches the plugin
//! manifest's `mcp_servers[].name` (`x-api`).

use clap::Subcommand;
use psychological_operations_sdk::cli::Output;
use psychological_operations_x_api_mcp::Mode;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// X-API MCP server commands. Nested under the server's name so the
    /// objectiveai host's `mcp <name> begin` plugin-launch convention
    /// resolves here (`<name>` = `x-api`).
    #[command(name = "x-api")]
    XApi {
        #[command(subcommand)]
        command: XApiCommands,
    },
}

#[derive(Subcommand)]
pub enum XApiCommands {
    /// Run the X-API MCP server in-process. Binds a random localhost
    /// port, emits one JSONL line with the URL, then serves until the
    /// process is killed. Cache config (size + TTL) comes from the
    /// env-derived process `Context`, not flags. Per-session
    /// `(agent, mode)` are supplied by the client on connect via the
    /// `X-OBJECTIVEAI-ARGUMENTS` header (with
    /// `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the agent fallback),
    /// so the only flag is `--mode` — and that's discarded too (see
    /// below). Quota is per-account, per-tool-call, configured via
    /// `agents quota`.
    Begin {
        /// REQUIRED for launch-command compatibility, then DISCARDED.
        /// objectiveai appends `--mode <mode>` when it launches a plugin
        /// MCP server, but this server reads the session's mode (and
        /// agent) per-request from the `X-OBJECTIVEAI-ARGUMENTS` header,
        /// not from this flag. Required (no default) and validated
        /// strictly: only the real `Mode` values (`readonly` / `full`)
        /// parse.
        #[arg(long, value_enum)]
        mode: Mode,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::XApi { command } => command.handle(ctx).await,
        }
    }
}

impl XApiCommands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                // `mode` is accepted (and validated) only for launch-
                // command compatibility — the server uses the per-session
                // header value, so we discard it here.
                XApiCommands::Begin { mode: _ } => {
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
                        ctx.cache_max_size,
                        ctx.cache_ttl,
                        ctx.config.mock,
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
