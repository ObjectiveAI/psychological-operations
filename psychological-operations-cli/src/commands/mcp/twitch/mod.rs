//! `mcp twitch` subcommands — the Twitch MCP server (manifest name `twitch`).
//!
//! `mcp twitch begin` runs the server in-process (this CLI process becomes the
//! MCP server). See the parent `mcp` module docs for how the objectiveai host
//! launches it (`<plugin-exec> mcp twitch begin …`) and consumes the emitted
//! `Output::Mcp` URL line. Mirrors `mcp discord begin`; the session arguments
//! are the same (`mode`, `tag`, `quota_*`) — only the per-tool
//! `quota_usage_<tool>` set differs (Twitch's four tools).

use clap::Subcommand;
use objectiveai_sdk::cli::command::agents::tags::apply as tags_apply;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::twitch;
use psychological_operations_twitch_mcp::Mode;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the Twitch MCP server in-process. First it makes the agent
    /// auth-ready: resolve the agent's user token and validate it
    /// (`id.twitch.tv/oauth2/validate`), aborting if that fails; then bind
    /// `tag` to the caller's own agent instance (`agents tags apply`, result
    /// ignored). Only then does it bind a random localhost port, emit one JSONL
    /// line with the URL, and serve until the process is killed.
    ///
    /// The per-session `mode` + optional `max_message_length` / `quota_*`
    /// overrides are supplied by the client on connect via the
    /// `X-OBJECTIVEAI-ARGUMENTS` header (which also re-carries `tag`); the flags
    /// below exist so the conduit's `mcp twitch begin --<arg> <value>` launch
    /// parses. `--tag` is consumed at startup; the rest are DISCARDED here,
    /// validated at connect-time in the header parser.
    Begin {
        /// DISCARDED (header-sourced). Kept as a `Mode` value-enum so a launch
        /// with a bogus `--mode` still fails fast.
        #[arg(long, value_enum)]
        mode: Mode,

        /// REQUIRED. The agent tag the session acts as. Consumed at startup
        /// (auth check + tag binding) before the server serves.
        #[arg(long)]
        tag: String,

        /// DISCARDED (header-sourced). Max message length; parsed + validated
        /// per session from the header (Twitch default 500).
        #[arg(long = "max_message_length")]
        max_message_length: Option<String>,

        // Optional per-session quota overrides — accepted as opaque strings
        // (NOT validated here) only so the conduit's `begin --<k> <v>` launch
        // parses; DISCARDED, re-parsed per session from the header.
        #[arg(long = "quota_read")]
        quota_read: Option<String>,
        #[arg(long = "quota_write")]
        quota_write: Option<String>,
        #[arg(long = "quota_interval")]
        quota_interval: Option<String>,
        #[arg(long = "quota_usage_whoami")]
        quota_usage_whoami: Option<String>,
        #[arg(long = "quota_usage_list_channels")]
        quota_usage_list_channels: Option<String>,
        #[arg(long = "quota_usage_list_messages")]
        quota_usage_list_messages: Option<String>,
        #[arg(long = "quota_usage_send_message")]
        quota_usage_send_message: Option<String>,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                // `tag` is consumed at startup (auth check + tag apply); the
                // rest are accepted only for launch-command compatibility and
                // discarded — the server reads them per-session from the
                // `X-OBJECTIVEAI-ARGUMENTS` header (validated at connect).
                Commands::Begin { tag, .. } => {
                    // 1. Auth-readiness gate. The Twitch client must be able to
                    //    act as this agent: resolve its user token and validate
                    //    it. Any error aborts before the server binds (mock mode
                    //    skips the network check).
                    if !ctx.config.mock {
                        let client =
                            twitch::Client::new(ctx.db.clone(), ctx.cache_max_size, ctx.cache_ttl);
                        client.validate(&tag).await.map_err(|e| {
                            Error::Other(format!("agent {tag} auth check (twitch validate): {e}"))
                        })?;
                    }

                    // 2. Bind `tag` to the caller's OWN agent instance so
                    //    tag-addressed notifications resolve here. Identical to
                    //    `mcp discord begin`.
                    let caller_aih = &ctx.config.objectiveai_agent_instance_hierarchy;
                    let (parent, agent_instance) =
                        caller_aih.rsplit_once('/').ok_or_else(|| {
                            Error::Other("caller must be an objectiveai agent".to_string())
                        })?;
                    let apply = tags_apply::Request {
                        path_type: tags_apply::Path::AgentsTagsApply,
                        name: tag.clone(),
                        target: tags_apply::Target::AgentInstance {
                            agent_instance: agent_instance.to_string(),
                            parent_agent_instance_hierarchy: Some(parent.to_string()),
                        },
                        base: Default::default(),
                    };
                    let _ = tags_apply::execute(&*ctx.executor, apply, None).await;

                    // 3. Serve until torn down.
                    psychological_operations_twitch_mcp::run(
                        "127.0.0.1",
                        0,
                        ctx.db.clone(),
                        ctx.cache_max_size,
                        ctx.cache_ttl,
                    )
                    .await
                    .map_err(|e| Error::Other(format!("mcp run: {e}")))?;
                    Ok(Output::Ok)
                }
            }
        }
        .await;
        crate::output::emit_result(result)
    }
}
