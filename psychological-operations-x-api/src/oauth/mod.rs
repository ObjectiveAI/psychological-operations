//! Per-psyop X OAuth 2.0 PKCE user-context flow — wire-layer
//! pieces only. The CLI subcommand (`psyops oauth <name>`) that
//! orchestrates these into a full flow lives in
//! `psychological-operations-cli` (`oauth_setup.rs`) because it
//! depends on the CLI's chromium / config / event-emit modules.
//!
//! - `pkce`: PKCE pair + state nonce generation
//! - `server`: one-shot localhost callback listener (OS-assigned port)
//! - `tokens`: token-endpoint wire helpers (exchange + refresh).
//!   On-disk token storage moved to
//!   `psychological_operations_sdk::browser::auth_json`.

pub mod pkce;
pub mod server;
pub mod tokens;
