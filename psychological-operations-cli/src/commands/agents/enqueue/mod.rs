//! `agents enqueue` — flag an item for an agent's queue, then auto-notify.
//!
//! Parent command nesting one subcommand per platform: `x` (a tweet, into
//! the X queue) and `discord` (a message, into the Discord queue). Both are
//! per-agent (caller-agnostic) and addressed by `--agent-tag`.

use clap::Subcommand;

pub mod discord;
pub mod x;

#[derive(Subcommand)]
pub enum Commands {
    /// Flag a tweet for an agent's X queue, then notify the agent.
    #[command(name = "x")]
    X {
        /// Agent tag whose queue to add to.
        #[arg(long)]
        agent_tag: String,
        /// Numeric ID of the tweet.
        #[arg(long)]
        tweet_id: String,
        /// Free-text note for the agent. Required.
        #[arg(long)]
        message: String,
    },
    /// Flag a Discord message for an agent's Discord queue, then notify the
    /// agent. A Discord message needs both `--channel-id` and `--message-id`.
    #[command(name = "discord")]
    Discord {
        /// Agent tag whose queue to add to.
        #[arg(long)]
        agent_tag: String,
        /// The channel the message is in (snowflake id).
        #[arg(long)]
        channel_id: String,
        /// The message's snowflake id.
        #[arg(long)]
        message_id: String,
        /// Free-text note for the agent. Required.
        #[arg(long)]
        message: String,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::X {
                agent_tag,
                tweet_id,
                message,
            } => x::run(&agent_tag, &tweet_id, &message, ctx).await,
            Commands::Discord {
                agent_tag,
                channel_id,
                message_id,
                message,
            } => discord::run(&agent_tag, &channel_id, &message_id, &message, ctx).await,
        }
    }
}
