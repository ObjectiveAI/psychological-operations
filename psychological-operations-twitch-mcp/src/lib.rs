//! Psychological Operations Twitch MCP server library.
//!
//! Mirrors `psychological-operations-discord-mcp` in shape: other crates can
//! `use psychological_operations_twitch_mcp::{run, setup, serve}` and embed the
//! server in-process; the binary at `main.rs` is a thin clap wrapper that
//! parses args and calls [`run`]. The Twitch SDK client only needs the `db`
//! handle (no reqwest / state_dir / cache / mock), so the surface is slim.
//!
//! `tag`, `mode`, and the per-session `quota_*` overrides are sourced on every
//! connect from the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header — see
//! [`crate::twitch_api::session`] for the source-resolution contract.

mod header_session_manager;
mod mode;
mod run;
mod twitch_api;

pub use mode::Mode;
pub use run::{run, serve, setup};
pub use twitch_api::PsychologicalOperationsTwitchMcp;
pub use twitch_api::session::HEADER_ARGUMENTS;
pub use twitch_api::tool_name::{Direction, ToolName};
