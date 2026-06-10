//! `agents queue` subcommand surface — operator-side queue ops.

use clap::Subcommand;

pub mod add;

#[derive(Subcommand)]
pub enum Commands {
    /// Enqueue a tweet for the current agent (sourced from
    /// `OBJECTIVEAI_AGENT_ID`). The queue is per-agent
    /// (caller-agnostic).
    #[command(name = "add")]
    Add {
        /// Numeric ID of the tweet.
        #[arg(long)]
        tweet_id: String,
        /// Free-text note for the agent. Required.
        #[arg(long)]
        message: String,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Add { tweet_id, message } => add::run(&tweet_id, &message, ctx).await,
        }
    }
}
