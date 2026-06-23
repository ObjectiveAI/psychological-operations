//! Tool surface, split by side-effect class. Each file's
//! `#[tool_router(router = …, vis = "pub")]` impl block generates a `pub fn`
//! on the `PsychologicalOperationsDiscordMcp` impl that builds a
//! `ToolRouter<Self>`; `mod.rs::new` combines them with the rmcp `+` operator.
//!
//! - `read` / `write` — Discord API reads / mutations (metered).
//! - `queue` — the per-agent ingest queue (DB-only, quota-free).
//! - `other` — neither read nor write against Discord (no API call), e.g.
//!   `invite_link` (quota-free).

pub mod other;
pub mod queue;
pub mod read;
pub mod write;
