use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One channel-read source: paginate a Discord channel's message history
/// (`GET /channels/{channel_id}/messages`) as the given bot.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Channel {
    /// The channel to read messages from (snowflake id). Works for any
    /// channel the bot can see — guild text channels and threads.
    pub channel_id: String,
    /// The agent (bot) whose token reads the channel. Required — the read
    /// acts as this agent, and the run-time pre-flight refuses the psyop if
    /// the agent isn't authed.
    pub agent_tag: String,
    /// Max messages to pull from this channel. Required. The read paginates
    /// (100 per page) until it has this many messages or the history runs
    /// out. Must be > 0.
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

impl Channel {
    /// Publish-time validation.
    pub fn validate(&self) -> Result<(), String> {
        if self.channel_id.trim().is_empty() {
            return Err("channel_id must not be empty".into());
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
