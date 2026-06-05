//! `functions` subcommand surface — mirrors the `objectiveai
//! functions` subtree at the surface the operator sees. Only the
//! psyops-relevant leaves live here.

use clap::Subcommand;

pub mod invent;

#[derive(Subcommand)]
pub enum Commands {
    /// Recursive function invention. Args mirror
    /// `objectiveai functions inventions recursive create`
    /// verbatim — same flags, same stream-routing
    /// (`--dangerous-advanced '{"stream":true}'` opts in). Psyops
    /// auto-applies its post input-schema; there is no schema
    /// flag.
    Invent {
        #[command(subcommand)]
        command: invent::Commands,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Invent { command } => command.handle(ctx).await,
        }
    }
}
