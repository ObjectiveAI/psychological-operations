//! The view of a tweet that filtering and sorting operate on.
//!
//! `db::Post` is the persistence shape (engagement metadata + body
//! text + media URLs); `Tweet` is the runtime shape (engagement
//! metadata + precomputed `age`, no content) used by
//! `psyops::Filter::evaluate` and `psyops::SortBy::evaluate`.
//! Content lives in `db::contents` and is not loaded for filtering /
//! sorting paths.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweet {
    pub id: String,
    pub handle: String,
    /// RFC 3339. Kept on the struct so `SortBy::Newest` / `Oldest`
    /// can sort lexically and the Custom-sort Python expression
    /// can reach it.
    pub created: String,
    /// Seconds since `created`. Computed once at hydration time so
    /// `Filter::evaluate` doesn't need an ambient `now`.
    pub age: u64,
    pub likes: u64,
    pub retweets: u64,
    pub replies: u64,
    pub impressions: u64,
}

/// Convert a `&Tweet` into a JSON dict with stable string keys — the
/// element shape the `SortBy::Python` Python expression sees in its
/// `input` list.
pub fn tweet_json(t: &Tweet) -> serde_json::Value {
    serde_json::json!({
        "id": t.id,
        "handle": t.handle,
        "created": t.created,
        "age": t.age,
        "likes": t.likes,
        "retweets": t.retweets,
        "replies": t.replies,
        "impressions": t.impressions,
    })
}

/// Build the per-post JSON dict for `OutputTop::Python`. Same key set as
/// [`tweet_json`] plus `score`. The post is the `db::Post` shape (no
/// precomputed `age`), so we recompute age against the supplied `now` using
/// the same helper db hydration uses — keeps `age` semantics consistent with
/// what `SortBy::Python` sees.
pub fn post_with_score_json(
    p: &crate::db::Post,
    score: f64,
    now: &chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    let age = crate::db::compute_age(&p.created, now);
    serde_json::json!({
        "id": p.id,
        "handle": p.handle,
        "created": p.created,
        "age": age,
        "likes": p.likes,
        "retweets": p.retweets,
        "replies": p.replies,
        "impressions": p.impressions,
        "score": score,
    })
}

#[cfg(test)]
pub(crate) fn tw_default(id: &str) -> Tweet {
    Tweet {
        id: id.into(),
        handle: "anon".into(),
        created: "2026-01-01T00:00:00Z".into(),
        age: 0,
        likes: 0,
        retweets: 0,
        replies: 0,
        impressions: 0,
    }
}
