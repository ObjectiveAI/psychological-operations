//! Tool surface, split by side-effect class. Each file's
//! `#[tool_router(router = …, vis = "pub")]` impl block generates a `pub fn`
//! on the `PsychologicalOperationsTwitchMcp` impl that builds a
//! `ToolRouter<Self>`; `mod.rs::new` combines them with the rmcp `+` operator.
//!
//! - `read` — Twitch reads (whoami/validate + the postgres chat buffer +
//!   channel-join set; metered against the read budget).
//! - `write` — Twitch chat send via Helix (metered against the write budget,
//!   full-only).

pub mod read;
pub mod write;
