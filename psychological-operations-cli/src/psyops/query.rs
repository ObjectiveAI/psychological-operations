use serde::{Deserialize, Serialize};

/// One live X v2 search-query input on a psyop. Carries its own
/// per-tweet eligibility fields (min_likes / min_age / etc.) directly.
#[derive(Debug, Serialize, Deserialize, Clone)]
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

    // ----- per-tweet eligibility (was the shared Filter struct) -----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_likes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_retweets: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_replies: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_impressions: Option<u64>,
    /// Reject tweets whose `created` is older than this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u64>,
    /// Reject tweets whose `created` is younger than this many seconds.
    /// Useful for letting engagement settle before scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_age: Option<u64>,
}

/// Which X v2 search endpoint a `Query` should hit.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchEndpoint {
    /// `/2/tweets/search/recent` — last 7 days, all access tiers.
    Recent,
    /// `/2/tweets/search/all` — full archive (Pro / Enterprise tiers).
    All,
}

impl Default for SearchEndpoint {
    fn default() -> Self { Self::Recent }
}
