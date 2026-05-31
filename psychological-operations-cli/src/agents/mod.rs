//! `agents` subcommand surface.
//!
//! Agents are X accounts the operator controls but doesn't browse as
//! a human — they're the "act" side of the pipeline, opposite the
//! psyops "read" side. Unlike psyops, agents can share the same
//! logged-in user (the twid-conflict guard does not fire) and have
//! no scrape mode.
//!
//! Today's surface: `agents oauth <name>` — spawn the embedded
//! browser in `--agent-authorize <name>` mode and let it write
//! `<base>/.../agent/<name>/handles/<twid>/auth.json` via the SDK's
//! `auth_json` module.

use clap::Subcommand;

use crate::browser::{extract::ensure_extracted, launch};
use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Authorize an agent's X account via OAuth 2.0 (PKCE). Opens
    /// the embedded browser scoped to `agent/<name>/`; on sign-in
    /// the browser drives the consent screen, exchanges the code,
    /// and writes auth.json under the agent's data root. Idempotent
    /// — re-running refreshes tokens.
    #[command(name = "oauth")]
    OAuth { name: String },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
        match self {
            Commands::OAuth { name } => oauth(&name, cfg).await,
        }
    }
}

async fn oauth(agent_name: &str, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted(cfg)?;
    let config_base_dir = cfg.objectiveai_base_dir();

    let mut child = launch::spawn(
        &materialized.binary,
        &config_base_dir,
        launch::Mode::AgentAuthorize { name: agent_name.to_string() },
        /* pipe_stdout = */ false,
    )?;

    crate::emit::emit(crate::events::Event::BrowserSpawned {
        kind: "agent_authorize".into(),
        name: Some(agent_name.to_string()),
        pid: child.id(),
    });

    let status = child.wait().map_err(|e| {
        Error::Other(format!("waiting for browser ({agent_name}) failed: {e}"))
    })?;

    crate::emit::emit(crate::events::Event::BrowserExit {
        kind: "agent_authorize".into(),
        name: Some(agent_name.to_string()),
        status: status.code(),
    });

    Ok(crate::Output::Empty)
}
