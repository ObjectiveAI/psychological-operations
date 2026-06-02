//! `agents queue` subcommand surface — operator-side queue ops.

use clap::Subcommand;

use crate::error::Error;

pub mod add;
pub mod executor;
pub mod handle;

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
    /// Walk the queue and hand each agent's pending work off to
    /// objectiveai. Runs all agents concurrently.
    #[command(name = "handle")]
    Handle {
        /// Restrict to specific X-API agents. Repeatable. When
        /// omitted, runs for every agent with ≥1 queued row.
        /// Agents with no rows are silently skipped (no
        /// notification emitted).
        #[arg(long)]
        agent: Vec<String>,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
        match self {
            Commands::Add { tweet_id, message } => add::run(&tweet_id, &message, cfg).await,
            Commands::Handle { agent } => handle::run(agent, cfg).await,
        }
    }
}
