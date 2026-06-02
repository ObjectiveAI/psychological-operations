//! Psychological Operations X-API MCP server library.
//!
//! Exposes [`Mode`] (so the binary's clap derive can use it) and
//! [`PsychologicalOperationsXApiMcp`] (the rmcp ServerHandler) for
//! anyone who wants to wire up the server differently. All runtime
//! configuration lives at the binary's clap args — there's no
//! Config struct, no env-var layer.

mod mode;
mod x_api;

pub use mode::Mode;
pub use x_api::PsychologicalOperationsXApiMcp;
