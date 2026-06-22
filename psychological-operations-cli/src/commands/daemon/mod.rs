//! `daemon` — the resident Discord gateway daemon.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol). It takes a process-singleton lock, then runs
//! the Discord daemon forever — one gateway listener per agent that has both
//! Discord auth and at least one hook. Never returns.

use clap::Subcommand;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the resident Discord gateway daemon (never returns).
    Begin,
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Begin => crate::output::emit_result(begin(ctx).await),
        }
    }
}

async fn begin(ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Process-singleton: a second daemon would open duplicate gateway
    // connections for the same bots (which Discord punishes). Bow out if held.
    let lock_dir = ctx.config.state_dir().join("locks");
    let _claim = objectiveai_sdk::lockfile::try_acquire(
        &lock_dir,
        "discord-daemon",
        &format!("pid {} discord daemon", std::process::id()),
    )
    .await
    .ok_or_else(|| Error::Other("the Discord daemon is already running".into()))?;

    // Runs forever (gateway loops run in background tasks); only returns on a
    // startup error. The lock claim is held in this frame for the daemon's life
    // (the OS reclaims it on process exit).
    psychological_operations_daemon::run(ctx.db.clone(), (*ctx.executor).clone())
        .await
        .map_err(|e| Error::Other(format!("discord daemon: {e}")))?;
    Ok(CliOutput::Ok)
}
