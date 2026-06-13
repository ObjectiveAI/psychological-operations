//! Error type for the X v2 API client.

use crate::x::types::Problem;

/// All failure modes of an HTTP call to the X v2 API.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to build the HTTP request (bad URL, bad header, etc.).
    #[error("request build error: {0}")]
    RequestBuild(reqwest::Error),

    /// Network / transport error during the request.
    #[error("http transport error: {0}")]
    Transport(reqwest::Error),

    /// Server returned a non-success status, body did not parse as a
    /// `Problem`. The body is captured as-is.
    #[error("bad status {code}: {body}")]
    BadStatus {
        code: reqwest::StatusCode,
        body: serde_json::Value,
    },

    /// Server returned a non-success status with an RFC 7807
    /// `application/problem+json` body that parsed cleanly.
    #[error(
        "problem ({}): {}",
        problem.title,
        problem.detail.as_deref().unwrap_or("")
    )]
    Problem {
        code: reqwest::StatusCode,
        problem: Problem,
    },

    /// Failed to deserialize a 2xx response body into the expected
    /// `Response` type. `serde_path_to_error` reports which field
    /// blew up.
    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_path_to_error::Error<serde_json::Error>),

    /// A persistence-layer failure (postgres, the advisory locker, or
    /// the Chromium cookie probe) surfaced through the db crate.
    #[error("db: {0}")]
    Db(#[from] psychological_operations_db::Error),

    /// The caller's hourly read/write API quota is exhausted —
    /// the request was rejected before any HTTP fired. The message
    /// is agent-facing: it names which quota, the limit, and when
    /// to retry.
    #[error("{0}")]
    QuotaExceeded(String),

    /// Catch-all for non-categorized errors (mock-x-api dispatch
    /// failures, etc.). Prefer the typed variants above when one
    /// fits.
    #[error("{0}")]
    Other(String),
}
