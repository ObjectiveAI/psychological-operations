//! `agents` subcommand surface.
//!
//! Agents are X accounts the operator controls but doesn't browse as
//! a human — they're the "act" side of the pipeline, opposite the
//! psyops "read" side. Unlike psyops, agents can share the same
//! logged-in user (the twid-conflict guard does not fire) and have
//! no scrape mode.
//!
//! Both arms (`login`, `browser`) are thin dispatches into
//! `crate::login::run` / `crate::persona_browser::run` with
//! `PersonaKind::Agent`. There is no agent-specific business logic,
//! so there's no `crate::agents` module — this file is the entire
//! agent surface.

use clap::Subcommand;
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

pub mod queue;

#[derive(Subcommand)]
pub enum Commands {
    /// Sign in an agent's X account. Requires the master X-App to
    /// already be signed in + fully set up (`x_app setup`). Opens
    /// the embedded browser scoped to `agent/<name>/`; on sign-in
    /// the browser drives the OAuth 2.0 PKCE consent screen,
    /// exchanges the code, and writes auth.json under the agent's
    /// data root. Refuses if the agent is already signed in or
    /// already has an auth.json for the current X-App — pass
    /// `--dangerously-reset` to wipe its browser folder and re-login.
    #[command(name = "login")]
    Login {
        name: String,
        /// Wipe any existing browser state for this agent before
        /// signing in. Required when re-logging in for an agent
        /// that already has an active session or stored auth.json.
        #[arg(long)]
        dangerously_reset: bool,
    },
    /// Open the embedded browser as this agent. Loads x.com under
    /// the agent's CEF profile (shared with `agents login`). No
    /// OAuth flow, no scraping — just a clean browser. The
    /// operator closes the window when done; the CLI blocks on
    /// that exit. The only mode hint shown is "Sign in to X" if
    /// not signed in.
    #[command(name = "browser")]
    Browser {
        name: String,
    },
    /// Per-(operator, agent) tweet handling queue.
    #[command(name = "queue")]
    Queue {
        #[command(subcommand)]
        command: queue::Commands,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<Output, Error> {
        match self {
            Commands::Login { name, dangerously_reset } => {
                crate::login::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Agent,
                    &name,
                    dangerously_reset,
                    cfg,
                )
                .await
            }
            Commands::Browser { name } => {
                crate::persona_browser::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Agent,
                    &name,
                    cfg,
                )
                .await
            }
            Commands::Queue { command } => command.handle(cfg).await,
        }
    }
}
