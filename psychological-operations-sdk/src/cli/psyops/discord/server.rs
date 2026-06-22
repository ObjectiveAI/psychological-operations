use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A server-read source: paginate across every channel of one Discord server
/// (guild) the bot can see. (For a single channel, use the
/// [`Channel`](super::channel::Channel) source.)
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Server {
    /// The server (guild) to read messages from (snowflake id). "Server" is
    /// the user-facing name for what the Discord API calls a guild.
    pub guild_id: String,
    /// The agent (bot) whose token reads the server. Required — the read acts
    /// as this agent, and the run-time pre-flight refuses the psyop if the
    /// agent isn't authed.
    pub agent_tag: String,
    /// Max messages to pull across the server's channels. Required. The read
    /// paginates (100 per page) until it has this many messages or the
    /// histories run out. Must be > 0.
    pub count: u64,
    /// Priority bucket for ordering the candidate union: smaller numbers
    /// come first; `None` ranks below every `Some(_)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    /// Optional Python boolean expression filtering each message before it
    /// becomes a candidate. `None` accepts every message the read returns.
    /// Not parse-checked at publish time — errors surface at run time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_filter: Option<String>,
}

impl Server {
    /// Publish-time validation.
    pub fn validate(&self) -> Result<(), String> {
        if self.guild_id.trim().is_empty() {
            return Err("guild_id must not be empty".into());
        }
        if self.agent_tag.trim().is_empty() {
            return Err("agent_tag must not be empty".into());
        }
        if self.count == 0 {
            return Err("count must be > 0".into());
        }
        Ok(())
    }
}
