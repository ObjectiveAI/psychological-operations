//! Psychological Operations X-API MCP server library.
//!
//! Other crates can `use psychological_operations_x_api_mcp::{run, setup, serve}`
//! and embed the server in-process. The binary at `main.rs` is a
//! thin clap wrapper that parses args and calls [`run`]. All
//! configuration is explicit at the function signatures — there
//! is no `Config` struct and no env-var layer.
//!
//! `agent` and `mode` are per-session, sourced on every connect
//! from the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header and (for
//! `agent` as fallback) the `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY`
//! header — see [`crate::x_api::session`] for the source-resolution
//! contract.

mod header_session_manager;
mod mode;
mod run;
mod x_api;

pub use mode::Mode;
pub use run::{run, serve, setup};
pub use x_api::PsychologicalOperationsXApiMcp;
pub use x_api::session::{HEADER_AGENT_INSTANCE_HIERARCHY, HEADER_ARGUMENTS};
