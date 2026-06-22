//! Tool surface, split by side-effect class. Each file's
//! `#[tool_router(router = …, vis = "pub")]` impl block generates a `pub fn`
//! on the `PsychologicalOperationsDiscordMcp` impl that builds a
//! `ToolRouter<Self>`; `mod.rs::new` combines them with the rmcp `+` operator
//! (`Self::read_tools() + Self::write_tools() + Self::queue_tools()`).
//!
//! Only `queue` is populated for now; `read` and `write` are empty routers
//! holding room for the tools to come.

pub mod queue;
pub mod read;
pub mod write;
