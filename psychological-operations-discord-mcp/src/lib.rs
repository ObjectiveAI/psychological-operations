//! Psychological Operations Discord MCP server library.
//!
//! Mirrors `psychological-operations-x-mcp` in shape: other crates can
//! `use psychological_operations_discord_mcp::{run, setup, serve}` and embed
//! the server in-process; the binary at `main.rs` is a thin clap wrapper that
//! parses args and calls [`run`]. The Discord SDK client only needs the `db`
//! handle (no reqwest / state_dir / cache / mock), so the surface is slimmer
//! than X.
//!
//! `tag`, `mode`, and the per-session `quota_*` overrides are sourced on every
//! connect from the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header — see
//! [`crate::discord_api::session`] for the source-resolution contract.

mod discord_api;
mod header_session_manager;
mod mode;
mod run;

pub use discord_api::PsychologicalOperationsDiscordMcp;
pub use discord_api::session::HEADER_ARGUMENTS;
pub use discord_api::tool_name::{Direction, ToolName};
pub use mode::Mode;
pub use run::{run, serve, setup};
