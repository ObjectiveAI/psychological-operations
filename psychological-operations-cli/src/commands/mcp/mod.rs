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
use objectiveai_sdk::cli::command::agents::tags::apply as tags_apply;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::users::me as users_me;
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
    /// Run the X-API MCP server in-process. First it makes the agent
    /// auth-ready: fetch `/2/users/me` as `AuthMode::Agent(tag)` and
    /// abort if that fails, then (best-effort, result ignored) bind
    /// `tag` to an instance under the caller's AIH via `agents tags
    /// apply`. Only then does it bind a random localhost port, emit one
    /// JSONL line with the URL, and serve until the process is killed.
    /// Cache config (size + TTL) comes from the env-derived process
    /// `Context`, not flags. The per-session `mode` + optional `quota_*`
    /// overrides are supplied by the client on connect via the
    /// `X-OBJECTIVEAI-ARGUMENTS` header (which also re-carries `tag`);
    /// the flags below exist so the conduit's `mcp x-api begin --<arg>
    /// <value>` launch (one flag per declared argument) parses. `--tag`
    /// is consumed at startup as above; `--mode` + `--quota_*` are
    /// DISCARDED here, validated at connect-time in the header parser.
    /// Quota is per-tag, per-tool-call.
    Begin {
        /// DISCARDED (header-sourced). Kept as a `Mode` value-enum so a
        /// launch with a bogus `--mode` still fails fast; the real
        /// per-session mode is read from the header.
        #[arg(long, value_enum)]
        mode: Mode,

        /// REQUIRED. The agent tag the session acts as. Consumed at
        /// startup — the `/2/users/me` auth check + the `agents tags
        /// apply` binding — before the server serves. Per-request, the
        /// session still reads `tag` from the header, not this flag.
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
                // `tag` is consumed at startup (auth check + tag apply);
                // `mode` + `quota_*` are accepted only for launch-command
                // compatibility and discarded — the server reads them
                // per-session from the `X-OBJECTIVEAI-ARGUMENTS` header
                // (validated at connect).
                XApiCommands::Begin { tag, .. } => {
                    // 1. Auth-readiness gate. The X client must be able to
                    //    act as this agent: fetch `/2/users/me` as
                    //    `AuthMode::Agent(tag)`. Any error aborts before the
                    //    server binds (mock mode short-circuits to success).
                    let http = Client::new(
                        reqwest::Client::new(),
                        ctx.config.mock,
                        ctx.cache_max_size,
                        ctx.cache_ttl,
                        ctx.config.state_dir(),
                        ctx.db.clone(),
                    );
                    let auth = AuthMode::Agent(tag.clone());
                    let me_req = users_me::get::Request {
                        user_fields: Some(vec![params::UserFields::Username]),
                        expansions: None,
                        tweet_fields: None,
                    };
                    users_me::http::get(&http, &auth, &me_req)
                        .await
                        .map_err(|e| {
                            Error::Other(format!("agent {tag} auth check (users/me): {e}"))
                        })?;

                    // 2. Best-effort tag binding: apply `tag` to an instance
                    //    under the caller's AIH so later tag-addressed
                    //    notifications resolve here. Awaited to completion
                    //    but the result is deliberately ignored — pass or
                    //    fail, we go on to serve.
                    let apply = tags_apply::Request {
                        path_type: tags_apply::Path::AgentsTagsApply,
                        name: tag.clone(),
                        target: tags_apply::Target::AgentInstance {
                            agent_instance: tag.clone(),
                            parent_agent_instance_hierarchy: Some(
                                ctx.config.objectiveai_agent_instance_hierarchy.clone(),
                            ),
                        },
                        base: Default::default(),
                    };
                    let _ = tags_apply::execute(&*ctx.executor, apply, None).await;

                    // 3. Serve until torn down.
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
