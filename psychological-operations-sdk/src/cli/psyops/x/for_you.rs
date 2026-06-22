use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::filter::Filter;

/// Personalized "For You" timeline input on a psyop. Ingestion
/// mechanism is TBD — the X v2 API has no public algorithmic-feed
/// endpoint; the most likely candidate is the chronological home
/// timeline `/2/users/{id}/timelines/reverse_chronological`.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct ForYou {
    /// The agent whose For You feed this entry collects. Required — the
    /// browser opens this agent's profile to scrape, and the run-time
    /// pre-flight refuses the psyop if the agent isn't authed. Multiple
    /// `ForYou` entries (one per agent) collect into the same psyop;
    /// entries sharing a `priority` are round-robin interwoven at sort.
    pub agent_tag: String,
    /// Priority bucket for ordering the candidate union: smaller numbers
    /// come first; `None` ranks below every `Some(_)`. for_you entries
    /// sharing a priority are round-robin interwoven across agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    /// Per-tweet eligibility filter. `None` means accept every tweet
    /// the timeline returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
}

impl ForYou {
    /// Publish-time validation: the filter (if present) must be
    /// internally consistent.
    pub fn validate(&self) -> Result<(), String> {
        if self.agent_tag.trim().is_empty() {
            return Err("agent_tag must not be empty".into());
        }
        if let Some(f) = &self.filter {
            f.validate().map_err(|e| format!("filter: {e}"))?;
        }
        Ok(())
    }
}
