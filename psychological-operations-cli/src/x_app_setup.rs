//! `x_app setup` — open the embedded browser against the X-App
//! profile so the operator can sign into X, create their X app on
//! console.x.com, and let the browser's helpers capture credentials.
//!
//! The browser binary handles the entire dance (cookies watcher,
//! post-create-dialog HTML snapshot, OAuth-popup HTML snapshot,
//! disk writes via the SDK's `x_app_credentials` module). The CLI
//! just spawns it and waits.

use clap::Subcommand;

use crate::browser::{extract::ensure_extracted, launch};
use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Open the embedded browser against the master X-App profile.
    /// The operator signs into X, configures console.x.com, and the
    /// browser's helpers capture the OAuth 2.0 + bearer credentials
    /// into x_app.json.
    Setup,
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
        match self {
            Commands::Setup => run(cfg).await,
        }
    }
}

pub async fn run(cfg: &crate::run::Config) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted(cfg)?;
    let config_base_dir = cfg.objectiveai_base_dir();

    let mut child = launch::spawn(
        &materialized.binary,
        &config_base_dir,
        launch::Mode::XApp,
        /* pipe_stdin  = */ false,
        /* pipe_stdout = */ false,
    )?;

    crate::emit::emit(crate::events::Event::BrowserSpawned {
        kind: "x_app".into(),
        name: None,
        pid: child.id(),
    });

    let status = child.wait().map_err(|e| {
        Error::Other(format!("waiting for browser (x_app) failed: {e}"))
    })?;

    crate::emit::emit(crate::events::Event::BrowserExit {
        kind: "x_app".into(),
        name: None,
        status: status.code(),
    });

    Ok(crate::Output::Empty)
}
