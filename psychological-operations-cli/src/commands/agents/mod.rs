//! `agents` subcommand surface.
//!
//! Agents are X accounts the operator controls but doesn't browse as
//! a human — they're the "act" side of the pipeline, opposite the
//! psyops "read" side. Unlike psyops, agents can share the same
//! logged-in user (the twid-conflict guard does not fire) and have
//! no scrape mode.
//!
//! `login` / `browser` select their agent via the shared
//! [`agent_ref::AgentRef`] argument group; they're thin dispatches into
//! `crate::login::run` / `crate::persona_browser::run` with
//! `PersonaKind::Agent`. `enqueue` (by `--agent-tag`) parks a tweet in an
//! agent's queue and auto-notifies; `notify` is the shared
//! count-notification helper it (and `psyops run`) call into.

use clap::Subcommand;

mod agent_ref;
pub mod deliver;
pub mod enqueue;
pub mod invite;
pub mod login;
pub mod notify;
pub mod quota;

use agent_ref::AgentRef;

#[derive(Subcommand)]
pub enum Commands {
    /// Sign in an agent on a platform. Parent command: `login x`
    /// (X OAuth authorize) and `login discord` (the Discord
    /// bot-creation wizard). See each subcommand for specifics.
    #[command(name = "login")]
    Login {
        #[command(subcommand)]
        command: login::Commands,
    },
    /// Open the embedded browser as this agent. Loads x.com under
    /// the agent's CEF profile (shared with `agents login`). No
    /// OAuth flow, no scraping — just a clean browser. The
    /// operator closes the window when done; the CLI blocks on
    /// that exit. The only mode hint shown is "Sign in to X" if
    /// not signed in.
    #[command(name = "browser")]
    Browser {
        #[command(flatten)]
        agent: AgentRef,
    },
    /// Enqueue a tweet into an agent's per-agent queue (by tag), then
    /// auto-notify the agent of its new pending count.
    #[command(name = "enqueue")]
    Enqueue {
        /// Agent tag whose queue to add to.
        #[arg(long)]
        agent_tag: String,
        /// Numeric ID of the tweet.
        #[arg(long)]
        tweet_id: String,
        /// Free-text note for the agent. Required.
        #[arg(long)]
        message: String,
    },
    /// Output an invite link for an agent on a platform: `invite discord`
    /// prints the bot's Discord server-invite URL.
    #[command(name = "invite")]
    Invite {
        #[command(subcommand)]
        command: invite::Commands,
    },
    /// Quota management for an agent (`grant`).
    #[command(name = "quota")]
    Quota {
        #[command(subcommand)]
        command: quota::Commands,
    },
    /// Deliver pending reply/quote queue entries via the browser,
    /// removing each row as the browser confirms it.
    #[command(name = "deliver")]
    Deliver,
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Login { command } => command.handle(ctx).await,
            Commands::Browser { agent } => {
                let name = agent.resolve_raw(&ctx.config);
                crate::persona_browser::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Agent,
                    &name,
                    ctx,
                )
                .await
            }
            Commands::Enqueue {
                agent_tag,
                tweet_id,
                message,
            } => enqueue::run(&agent_tag, &tweet_id, &message, ctx).await,
            Commands::Invite { command } => command.handle(ctx).await,
            Commands::Quota { command } => command.handle(ctx).await,
            Commands::Deliver => deliver::run(ctx).await,
        }
    }
}
