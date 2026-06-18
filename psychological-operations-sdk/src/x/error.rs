//! Error type for the X v2 API client.

use crate::x::types::Problem;

/// Failure modes encountered while **obtaining authorization** for an X
/// request — i.e. everything that happens *before* the authorized request
/// is sent: resolving the persona from the browser cookie jar, reading or
/// refreshing the stored OAuth token, and loading the X-App config.
///
/// Kept distinct from a failure of the authorized request itself (see the
/// other [`Error`] variants): these are "the system / setup is broken"
/// — not something the calling agent can fix by changing its request — so
/// callers (e.g. the X-API MCP) surface [`Error::Authorization`] as a hard
/// error rather than agent-facing tool output.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The required account isn't signed in to its browser profile (the
    /// cookie lookup returned nothing). String names which identity.
    #[error("not signed in: {0}")]
    NotSignedIn(String),

    /// The browser cookie probe itself failed — I/O, SQLite, decryption,
    /// or key material — i.e. a broken profile, not merely "not signed in".
    #[error("cookie probe: {0}")]
    Cookie(String),

    /// No stored token row for this persona yet — the OAuth flow hasn't
    /// been completed. String names the persona.
    #[error("no stored token for {0} — complete the OAuth flow first")]
    NoTokens(String),

    /// The stored token has no `refresh_token` to refresh against.
    #[error("stored token has no refresh_token")]
    NoRefreshToken,

    /// The X-App OAuth client isn't configured (`client_id` /
    /// `client_secret` / `bearer_token` missing) — run `x-app setup`.
    #[error("X-App not configured: {0}")]
    XAppNotConfigured(String),

    /// The OAuth token-refresh request to X failed (transport or status).
    #[error("token refresh failed: {0}")]
    Refresh(String),

    /// A persistence-layer failure while reading/writing the token row,
    /// loading config, or acquiring the auth lock.
    #[error("auth store: {0}")]
    Store(#[from] psychological_operations_db::Error),

    /// Stored-token JSON failed to encode or parse.
    #[error("stored token serde: {0}")]
    TokenSerde(String),

    /// A persona auth method was called for `AuthMode::XApp`, which has no
    /// persona (programmer error — unreachable from the MCP).
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// All failure modes of an HTTP call to the X v2 API.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to obtain authorization for the request (persona/cookie
    /// resolution, token read/refresh, or X-App config) — see
    /// [`AuthError`]. Distinct from a failure of the authorized request
    /// itself: the credentials/setup are at fault, not the request.
    #[error("authorization: {0}")]
    Authorization(AuthError),
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

    /// Catch-all for non-categorized errors (mock-x-api dispatch
    /// failures, etc.). Prefer the typed variants above when one
    /// fits.
    #[error("{0}")]
    Other(String),
}
