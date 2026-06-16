//! `x_app` subcommand surface.

use clap::Subcommand;

pub mod setup;

#[derive(Subcommand)]
pub enum Commands {
    /// Set up the X-App master OAuth credentials. Spawns the
    /// embedded browser scoped to `x-app/`; on sign-in the
    /// operator follows on-screen instructions to capture the
    /// post-create dialog + OAuth-popup snapshots.
    ///
    /// Refuses to run if the X-App is already fully set up
    /// (signed in + both snapshots complete) — pass
    /// `--dangerously-reset` to wipe the X-App folder AND every
    /// persona's auth.json (CEF cookies for personas stay) and
    /// start over.
    #[command(name = "setup")]
    Setup {
        /// Wipe X-App + every persona's auth folder before
        /// launching. Required if the X-App is already signed in
        /// AND both snapshots are complete.
        #[arg(long)]
        dangerously_reset: bool,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Setup { dangerously_reset } => setup::run(dangerously_reset, ctx).await,
        }
    }
}
