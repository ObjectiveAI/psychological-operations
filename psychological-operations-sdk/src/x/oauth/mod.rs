//! Per-persona X OAuth 2.0 PKCE user-context flow — wire-layer
//! pieces only. The CLI subcommands (`psyops login <name>` and
//! `agents login <name>`) that orchestrate these into a full
//! flow live in `psychological-operations-cli` (`login.rs`)
//! because they depend on the CLI's chromium / config /
//! event-emit modules.
//!
//! - `pkce`: PKCE pair + state nonce generation
//! - `server`: one-shot localhost callback listener (OS-assigned port)
//! - `tokens`: token-endpoint wire helpers (exchange + refresh).
//!   On-disk token storage moved to
//!   `psychological_operations_sdk::browser::auth_json`.

pub mod pkce;
pub mod server;
pub mod tokens;
