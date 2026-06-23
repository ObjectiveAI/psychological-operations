//! `mcp discord` subcommands — the Discord MCP server (manifest name
//! `discord`).
//!
//! `mcp discord begin` runs the server in-process (this CLI process becomes
//! the MCP server). See the parent `mcp` module docs for how the objectiveai
//! host launches it (`<plugin-exec> mcp discord begin …`) and consumes the
//! emitted `Output::Mcp` URL line. Mirrors `mcp x begin`; the session
//! arguments are identical (`mode`, `tag`, `quota_read`/`quota_write`/
//! `quota_interval`) — only the per-tool `quota_usage_<tool>` arguments differ,
//! and there are none yet (queue tools are quota-free).

use clap::Subcommand;
use objectiveai_sdk::cli::command::agents::tags::apply as tags_apply;
use psychological_operations_discord_mcp::Mode;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::discord;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the Discord MCP server in-process. First it makes the agent
    /// auth-ready: resolve the agent's bot token and validate it
    /// (`/users/@me`), aborting if that fails; then bind `tag` to the caller's
    /// own agent instance (split the caller AIH on its last `/`; a caller AIH
    /// with no `/` is rejected) and `agents tags apply` it (result ignored).
    /// Only then does it bind a random localhost port, emit one JSONL line
    /// with the URL, and serve until the process is killed.
    ///
    /// The per-session `mode` + optional `quota_*` overrides are supplied by
    /// the client on connect via the `X-OBJECTIVEAI-ARGUMENTS` header (which
    /// also re-carries `tag`); the flags below exist so the conduit's
    /// `mcp discord begin --<arg> <value>` launch parses. `--tag` is consumed
    /// at startup; `--mode` + `--quota_*` are DISCARDED here, validated at
    /// connect-time in the header parser.
    Begin {
        /// DISCARDED (header-sourced). Kept as a `Mode` value-enum so a launch
        /// with a bogus `--mode` still fails fast.
        #[arg(long, value_enum)]
        mode: Mode,

        /// REQUIRED. The agent tag the session acts as. Consumed at startup
        /// (auth check + tag binding) before the server serves.
        #[arg(long)]
        tag: String,

        // Optional per-session quota overrides. Accepted as opaque strings
        // (NOT validated here) only so the conduit's `begin --<k> <v>` launch
        // parses; DISCARDED — each is parsed + validated from the
        // `X-OBJECTIVEAI-ARGUMENTS` header per session. No `quota_usage_<tool>`
        // flags yet (no metered Discord tools).
        #[arg(long = "quota_read")]
        quota_read: Option<String>,
        #[arg(long = "quota_write")]
        quota_write: Option<String>,
        #[arg(long = "quota_interval")]
        quota_interval: Option<String>,
        #[arg(long = "quota_usage_whoami")]
        quota_usage_whoami: Option<String>,
        #[arg(long = "quota_usage_list_servers")]
        quota_usage_list_servers: Option<String>,
        #[arg(long = "quota_usage_list_channels")]
        quota_usage_list_channels: Option<String>,
        #[arg(long = "quota_usage_list_users")]
        quota_usage_list_users: Option<String>,
        #[arg(long = "quota_usage_list_role_members")]
        quota_usage_list_role_members: Option<String>,
        #[arg(long = "quota_usage_get_role")]
        quota_usage_get_role: Option<String>,
        #[arg(long = "quota_usage_list_available_reactions")]
        quota_usage_list_available_reactions: Option<String>,
        #[arg(long = "quota_usage_get_message_reactions_by_user")]
        quota_usage_get_message_reactions_by_user: Option<String>,
        #[arg(long = "quota_usage_list_messages")]
        quota_usage_list_messages: Option<String>,
        #[arg(long = "quota_usage_get_message")]
        quota_usage_get_message: Option<String>,
        #[arg(long = "quota_usage_get_user")]
        quota_usage_get_user: Option<String>,
        #[arg(long = "quota_usage_get_profile_picture")]
        quota_usage_get_profile_picture: Option<String>,
        #[arg(long = "quota_usage_open_attachment")]
        quota_usage_open_attachment: Option<String>,
        #[arg(long = "quota_usage_send_message")]
        quota_usage_send_message: Option<String>,
        #[arg(long = "quota_usage_send_direct_message")]
        quota_usage_send_direct_message: Option<String>,
        #[arg(long = "quota_usage_edit_message")]
        quota_usage_edit_message: Option<String>,
        #[arg(long = "quota_usage_delete_message")]
        quota_usage_delete_message: Option<String>,
        #[arg(long = "quota_usage_add_reaction")]
        quota_usage_add_reaction: Option<String>,
        #[arg(long = "quota_usage_remove_reaction")]
        quota_usage_remove_reaction: Option<String>,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                // `tag` is consumed at startup (auth check + tag apply); `mode`
                // + `quota_*` are accepted only for launch-command compatibility
                // and discarded — the server reads them per-session from the
                // `X-OBJECTIVEAI-ARGUMENTS` header (validated at connect).
                Commands::Begin { tag, .. } => {
                    // 1. Auth-readiness gate. The Discord client must be able to
                    //    act as this agent: resolve its bot token and validate
                    //    it via `/users/@me`. Any error aborts before the server
                    //    binds (mock mode skips the network check).
                    if !ctx.config.mock {
                        let client = discord::Client::new(ctx.db.clone());
                        let http = client.http(&tag).await.map_err(|e| {
                            Error::Other(format!("agent {tag} discord auth: {e}"))
                        })?;
                        http.get_current_user().await.map_err(|e| {
                            Error::Other(format!(
                                "agent {tag} auth check (discord /users/@me): {e}"
                            ))
                        })?;
                    }

                    // 2. Bind `tag` to the caller's OWN agent instance so
                    //    tag-addressed notifications resolve here. Identical to
                    //    `mcp x begin`.
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
                    psychological_operations_discord_mcp::run("127.0.0.1", 0, ctx.db.clone())
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
