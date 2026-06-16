use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::filter::Filter;

/// One live X v2 search-query input on a psyop.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct Query {
    /// X v2 search-operator string (e.g. `"from:user has:media -is:retweet"`).
    pub query: String,
    #[serde(default)]
    pub endpoint: SearchEndpoint,
    /// Higher = preferred when the deduped union is truncated by
    /// `PsyOp.max_posts`. `None` ranks below every `Some(_)`,
    /// regardless of the `Some` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u64>,
    /// Per-tweet eligibility filter. `None` means accept every tweet
    /// the search returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
}

impl Query {
    /// Publish-time validation: the search query string must be
    /// non-empty after trim, and the filter (if present) must be
    /// internally consistent.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("query string must not be empty".into());
        }
        if let Some(f) = &self.filter {
            f.validate().map_err(|e| format!("filter: {e}"))?;
        }
        Ok(())
    }
}

/// Which X v2 search endpoint a `Query` should hit.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchEndpoint {
    /// `/2/tweets/search/recent` — last 7 days, all access tiers.
    Recent,
    /// `/2/tweets/search/all` — full archive (Pro / Enterprise tiers).
    All,
}

impl Default for SearchEndpoint {
    fn default() -> Self {
        Self::Recent
    }
}
