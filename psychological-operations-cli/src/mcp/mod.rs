//! Embedded X-API MCP server: per-agent supervised launcher.
//!
//! The clap surface lives in `crate::commands::mcp`; the supervisor
//! (probe + spawn + state.json) lives in [`begin`]; the content-hashed
//! extract of the embedded binary lives in [`embed`].

pub mod begin;
pub mod embed;
