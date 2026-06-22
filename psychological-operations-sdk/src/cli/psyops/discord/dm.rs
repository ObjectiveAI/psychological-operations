use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One DM-read source: paginate the bot's direct-message history with one
/// specific user. The bot only ever sees DMs it is itself a party to — there
/// is no access to other users' DMs. To read every DM the bot has, use the
/// [`AllDms`](super::all_dms::AllDms) source instead.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Dm {
    /// The other party in the DM (user snowflake id). The bot's DM channel
    /// with this user is what gets read.
    pub user_id: String,
    /// The agent (bot) whose token reads the DM. Required — the read acts as
    /// this agent, and the run-time pre-flight refuses the psyop if the agent
    /// isn't authed.
    pub agent_tag: String,
    /// Max messages to pull. Required. The read paginates (100 per page)
    /// until it has this many messages or the history runs out. Must be > 0.
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

impl Dm {
    /// Publish-time validation.
    pub fn validate(&self) -> Result<(), String> {
        if self.user_id.trim().is_empty() {
            return Err("user_id must not be empty".into());
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
