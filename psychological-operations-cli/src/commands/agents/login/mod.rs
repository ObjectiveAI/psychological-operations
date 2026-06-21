//! `agents login` — platform sign-in subcommands.
//!
//! Parent command nesting one subcommand per platform: `x` (the X OAuth
//! 2.0 PKCE authorize flow) and `discord` (the Discord bot-creation
//! wizard). Both select their agent via the shared [`AgentRef`] group.

use clap::Subcommand;

pub mod discord;
pub mod x;

use super::agent_ref::AgentRef;

#[derive(Subcommand)]
pub enum Commands {
    /// Sign in an agent's X account. Requires the master X-App to already
    /// be set up (`x-app setup`). Opens the embedded browser scoped to the
    /// agent's CEF profile; on sign-in the browser drives the OAuth 2.0
    /// PKCE consent screen, exchanges the code, and stores the agent's
    /// tokens. `--dangerously-reset` wipes the agent's persona (tokens +
    /// CEF profile) and re-logs in.
    #[command(name = "x")]
    X {
        #[command(flatten)]
        agent: AgentRef,
        /// Wipe any existing X persona state for this agent before signing
        /// in. Required when re-logging in for an agent that already has a
        /// session or stored tokens.
        #[arg(long)]
        dangerously_reset: bool,
    },
    /// Create the agent's Discord bot. Opens the Discord developer portal
    /// (shared operator profile), guides sign-in + bot creation, scrapes
    /// the bot token, and stores it for the agent. `--dangerously-reset`
    /// drops the agent's stored bot token for a clean re-run.
    #[command(name = "discord")]
    Discord {
        #[command(flatten)]
        agent: AgentRef,
        /// Drop this agent's stored Discord bot token before re-running.
        #[arg(long)]
        dangerously_reset: bool,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::X {
                agent,
                dangerously_reset,
            } => {
                let name = agent.resolve_raw(&ctx.config);
                x::run(&name, dangerously_reset, ctx).await
            }
            Commands::Discord {
                agent,
                dangerously_reset,
            } => {
                let name = agent.resolve_raw(&ctx.config);
                discord::run(&name, dangerously_reset, ctx).await
            }
        }
    }
}
