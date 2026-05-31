//! `agents` subcommand surface.
//!
//! Agents are X accounts the operator controls but doesn't browse as
//! a human — they're the "act" side of the pipeline, opposite the
//! psyops "read" side. Unlike psyops, agents can share the same
//! logged-in user (the twid-conflict guard does not fire) and have
//! no scrape mode.
//!
//! Today's surface: `agents login <name>` — spawn the embedded
//! browser in `--agent-authorize <name>` mode and let it write
//! `<base>/.../agent/<name>/handles/<twid>/auth.json` via the SDK's
//! `auth_json` module. Routed through [`crate::login::run`], which
//! shares all pre-flight + browser-spawn logic with `psyops login`.

use clap::Subcommand;

use crate::error::Error;

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
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
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
        }
    }
}
