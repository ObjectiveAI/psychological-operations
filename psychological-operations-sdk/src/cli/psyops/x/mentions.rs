use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::filter::Filter;

/// A mentions ingestion source on a psyop. Reads the agent's
/// `/2/users/{id}/mentions` feed (posts mentioning the agent), paginating
/// up to `max_posts`. Sorts like a query (via `SortBy`), not interwoven
/// like for_you.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Mentions {
    /// The agent whose mentions this reads, acting as its auth
    /// (`AuthMode::Agent`). Required — the run-time pre-flight refuses the
    /// psyop if the agent isn't authed.
    pub agent_tag: String,
    /// Max posts to pull. Required. The read paginates (oldest pages last),
    /// taking from the top until it has this many or the pages run out.
    /// Must be > 0.
    pub max_posts: u64,
    /// Priority bucket for ordering the candidate union: smaller numbers
    /// come first; `None` ranks below every `Some(_)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    /// Per-tweet eligibility filter. `None` accepts every tweet returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
}

impl Mentions {
    pub fn validate(&self) -> Result<(), String> {
        if self.agent_tag.trim().is_empty() {
            return Err("agent_tag must not be empty".into());
        }
        if self.max_posts == 0 {
            return Err("max_posts must be > 0".into());
        }
        if let Some(f) = &self.filter {
            f.validate().map_err(|e| format!("filter: {e}"))?;
        }
        Ok(())
    }
}
