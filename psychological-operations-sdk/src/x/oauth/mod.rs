//! X OAuth 2.0 token-endpoint wire helper. Only [`tokens::refresh`]
//! survives here — it's called by [`crate::x::client::Client`] to
//! auto-refresh an expired access token. On-disk token storage lives in
//! [`crate::browser::auth_json`]. The rest of the per-persona authorize
//! flow (PKCE pair, localhost callback listener, initial code exchange)
//! is implemented in the browser crate (`psychological-operations-browser`),
//! not here.

pub mod tokens;
