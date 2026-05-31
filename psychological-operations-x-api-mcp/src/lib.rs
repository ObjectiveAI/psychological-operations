//! Psychological Operations X-API MCP server library.
//!
//! Other crates can `use psychological_operations_x_api_mcp::{ConfigBuilder, run}`
//! and spawn the server in-process; the binary at `main.rs` is a thin wrapper
//! that reads `Config` from the environment and calls [`run`].

mod run;
mod x_api;

pub use run::*;
