//! Master X dev-account / X-App credentials setup. The Chromium
//! extension captures credentials from `console.x.com` and ships
//! them via the native messaging port; the host writes them to
//! `~/.psychological-operations/x_app.json`. Per-psyop OAuth reads
//! that file to drive the user-context PKCE flow.

pub mod config;
pub mod setup;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// Open chromium against the master X-App profile. User signs
    /// into x.com / configures console.x.com / clicks the extension
    /// to save credentials to x_app.json.
    Setup,
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::Setup => setup::run(cfg).await,
        }
    }
}
