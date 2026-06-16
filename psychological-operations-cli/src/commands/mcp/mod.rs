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
    /// env-derived process `Context`, not flags. Every per-session value
    /// — `tag`, `mode`, and the optional `quota_*` overrides — is supplied
    /// by the client on connect via the `X-OBJECTIVEAI-ARGUMENTS` header.
    /// The matching flags below exist ONLY so the conduit's
    /// `mcp x-api begin --<arg> <value>` launch (one flag per declared
    /// argument) parses; they are all DISCARDED here, with the strict
    /// validation happening at connect-time in the header parser. Quota
    /// is per-tag, per-tool-call.
    Begin {
        /// DISCARDED (header-sourced). Kept as a `Mode` value-enum so a
        /// launch with a bogus `--mode` still fails fast; the real
        /// per-session mode is read from the header.
        #[arg(long, value_enum)]
        mode: Mode,

        /// REQUIRED, then DISCARDED. The agent tag the session acts as;
        /// read per-request from the header, not this flag. Present only
        /// so the conduit's `begin --tag <v>` launch parses.
        #[arg(long)]
        tag: String,

        // Optional per-session quota overrides. Accepted as opaque strings
        // (NOT validated here) only so the conduit's `begin --<k> <v>`
        // launch parses; DISCARDED — each is parsed + validated from the
        // `X-OBJECTIVEAI-ARGUMENTS` header per session, where a bad value
        // is a connect-time error. Underscored `long` names match the
        // verbatim `--<arg-key>` the conduit emits. `quota_interval` is a
        // humantime duration; the limits/costs are integers.
        #[arg(long = "quota_read")]
        quota_read: Option<String>,
        #[arg(long = "quota_write")]
        quota_write: Option<String>,
        #[arg(long = "quota_interval")]
        quota_interval: Option<String>,
        #[arg(long = "quota_usage_get_replies")]
        quota_usage_get_replies: Option<String>,
        #[arg(long = "quota_usage_get_bio")]
        quota_usage_get_bio: Option<String>,
        #[arg(long = "quota_usage_get_profile_picture")]
        quota_usage_get_profile_picture: Option<String>,
        #[arg(long = "quota_usage_get_tweet")]
        quota_usage_get_tweet: Option<String>,
        #[arg(long = "quota_usage_open_attachment")]
        quota_usage_open_attachment: Option<String>,
        #[arg(long = "quota_usage_run_query")]
        quota_usage_run_query: Option<String>,
        #[arg(long = "quota_usage_whoami")]
        quota_usage_whoami: Option<String>,
        #[arg(long = "quota_usage_get_bookmarks")]
        quota_usage_get_bookmarks: Option<String>,
        #[arg(long = "quota_usage_post")]
        quota_usage_post: Option<String>,
        #[arg(long = "quota_usage_reply")]
        quota_usage_reply: Option<String>,
        #[arg(long = "quota_usage_quote")]
        quota_usage_quote: Option<String>,
        #[arg(long = "quota_usage_like")]
        quota_usage_like: Option<String>,
        #[arg(long = "quota_usage_retweet")]
        quota_usage_retweet: Option<String>,
        #[arg(long = "quota_usage_bookmark")]
        quota_usage_bookmark: Option<String>,
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
                // Every flag is accepted only for launch-command
                // compatibility — the server reads each value per-session
                // from the `X-OBJECTIVEAI-ARGUMENTS` header (and validates
                // it at connect), so we discard them all here.
                XApiCommands::Begin { .. } => {
                    let state_dir = ctx.config.state_dir();
                    psychological_operations_x_api_mcp::run(
                        "127.0.0.1",
                        0,
                        state_dir,
                        ctx.db.clone(),
                        ctx.cache_max_size,
                        ctx.cache_ttl,
                        ctx.config.mock,
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
        }
        .await;
        crate::output::emit_result(result)
    }
}
