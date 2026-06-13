//! CLI-facing re-exports of the persistence layer.
//!
//! All storage now lives in `psychological-operations-db` (postgres).
//! This module used to own a SQLite `data.db`; that implementation is
//! gone. What remains is a thin facade so existing `crate::db::*` paths
//! keep resolving — the pipeline DTOs are re-exported from the db crate,
//! and `compute_age` (a pure, filesystem-free helper the CLI uses to
//! turn a `PostRow`'s `created` timestamp into an age) stays here next
//! to its callers.

pub use psychological_operations_db::{Db, MediaUrl, Origin, Post, PostRow, QueuedDelivery};

/// Parse `created` (RFC 3339) and return seconds since `now`. A
/// `created` that doesn't parse yields 0 — `min_age` filters would
/// reject it anyway, and we'd rather not error out the whole runtime
/// over one bad timestamp.
pub(crate) fn compute_age(created: &str, now: &chrono::DateTime<chrono::Utc>) -> u64 {
    match chrono::DateTime::parse_from_rfc3339(created) {
        Ok(t) => {
            let secs = (*now - t.with_timezone(&chrono::Utc)).num_seconds();
            secs.max(0) as u64
        }
        Err(_) => 0,
    }
}
