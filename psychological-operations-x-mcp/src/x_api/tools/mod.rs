//! Tool surface, split by side-effect class so each file stays
//! short. Each file's `#[tool_router(router = …, vis = "pub")]`
//! impl block generates a `pub fn` ON the
//! `PsychologicalOperationsXApiMcp` impl that builds a
//! `ToolRouter<Self>`; `mod.rs::new` combines them with the rmcp
//! `+` operator (`Self::read_tools() + Self::write_tools() +
//! Self::queue_tools()`).

pub mod queue;
pub mod read;
pub mod write;
