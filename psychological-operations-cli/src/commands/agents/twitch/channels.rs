//! `agents twitch channels {add,remove,list}` — the per-agent channel-join set
//! the daemon reads to decide which Twitch chats to JOIN and buffer.

use clap::Subcommand;
use psychological_operations_sdk::cli::Output as CliOutput;

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Add a channel to the agent's JOIN set (idempotent).
    Add {
        /// Agent tag whose Twitch auth the daemon listens as.
        #[arg(long)]
        agent_tag: String,
        /// Twitch channel login (the streamer's `#channel`; `#`/case ignored).
        #[arg(long)]
        channel: String,
    },
    /// Remove a channel from the agent's JOIN set.
    Remove {
        #[arg(long)]
        agent_tag: String,
        #[arg(long)]
        channel: String,
    },
    /// List the channels the agent JOINs.
    List {
        #[arg(long)]
        agent_tag: String,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        crate::output::emit_result(run(self, ctx).await)
    }
}

/// Twitch logins are lowercase; accept a leading `#` and any case.
fn normalize(channel: &str) -> String {
    channel.trim().trim_start_matches('#').to_lowercase()
}

async fn run(cmd: Commands, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    match cmd {
        Commands::Add { agent_tag, channel } => {
            ctx.db
                .twitch_channels_add(&agent_tag, &normalize(&channel))
                .await?;
            Ok(CliOutput::Ok)
        }
        Commands::Remove { agent_tag, channel } => {
            ctx.db
                .twitch_channels_remove(&agent_tag, &normalize(&channel))
                .await?;
            Ok(CliOutput::Ok)
        }
        Commands::List { agent_tag } => {
            let channels = ctx.db.twitch_channels_list(&agent_tag).await?;
            Ok(CliOutput::TwitchChannelList(channels))
        }
    }
}
