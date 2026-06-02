//! Tool surface, split by side-effect class so each file stays
//! short. The three `#[tool_router(router = …, vis = "pub")]`
//! impl blocks generate three free functions that
//! [`super::PsychologicalOperationsXApiMcp::new`] combines via the
//! rmcp `ToolRouter` `+` operator.

pub mod queue;
pub mod read;
pub mod write;

pub use queue::queue_tools;
pub use read::read_tools;
pub use write::write_tools;
