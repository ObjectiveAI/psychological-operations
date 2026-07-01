//! `agents` subcommand surface.
//!
//! Agents are X accounts the operator controls but doesn't browse as
//! a human — they're the "act" side of the pipeline, opposite the
//! psyops "read" side. Unlike psyops, agents can share the same
//! logged-in user (the twid-conflict guard does not fire) and have
//! no scrape mode.
//!
//! Every agent-addressing subcommand (`login`, `browser`, `invite`,
//! `enqueue`, `quota`) selects its agent by `--agent-tag`, used verbatim as
//! the name. `login` / `browser` are thin dispatches into `crate::login::run`
//! / `crate::persona_browser::run` with `PersonaKind::Agent`. `enqueue` parks
//! a tweet in an agent's queue and auto-notifies; `notify` is the shared
//! count-notification helper it (and `psyops run`) call into.

use clap::Subcommand;

pub mod daemon;
pub mod deliver;
pub mod enqueue;
pub mod invite;
pub mod login;
pub mod notify;
pub mod quota;
pub mod twitch;

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
        /// Agent tag, used verbatim as the name.
        #[arg(long)]
        agent_tag: String,
    },
    /// Enqueue an item into an agent's per-agent queue, then auto-notify the
    /// agent: `enqueue x` (a tweet) or `enqueue discord` (a message).
    #[command(name = "enqueue")]
    Enqueue {
        #[command(subcommand)]
        command: enqueue::Commands,
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
    /// Daemon management for an agent: `daemon discord hooks {add,list,delete}`
    /// manage the Python hooks the Discord gateway daemon runs.
    #[command(name = "daemon")]
    Daemon {
        #[command(subcommand)]
        command: daemon::Commands,
    },
    /// Deliver pending reply/quote queue entries via the browser,
    /// removing each row as the browser confirms it.
    #[command(name = "deliver")]
    Deliver,
    /// Twitch management for an agent: `twitch channels {add,remove,list}`
    /// manage which channels the daemon JOINs and buffers chat from.
    #[command(name = "twitch")]
    Twitch {
        #[command(subcommand)]
        command: twitch::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Login { command } => command.handle(ctx).await,
            Commands::Browser { agent_tag } => {
                crate::persona_browser::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Agent,
                    &agent_tag,
                    ctx,
                )
                .await
            }
            Commands::Enqueue { command } => command.handle(ctx).await,
            Commands::Invite { command } => command.handle(ctx).await,
            Commands::Quota { command } => command.handle(ctx).await,
            Commands::Daemon { command } => command.handle(ctx).await,
            Commands::Deliver => deliver::run(ctx).await,
            Commands::Twitch { command } => command.handle(ctx).await,
        }
    }
}
