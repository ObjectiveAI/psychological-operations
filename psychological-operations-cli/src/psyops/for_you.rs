use serde::{Deserialize, Serialize};

/// Personalized "For You" timeline input on a psyop. Carries its own
/// per-tweet eligibility fields directly (no shared Filter type).
///
/// Ingestion mechanism is TBD — the X v2 API has no public algorithmic-
/// feed endpoint; the most likely candidate is the chronological home
/// timeline `/2/users/{id}/timelines/reverse_chronological`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForYou {
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
