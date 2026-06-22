//! Write tools — Discord mutations. **Not implemented yet** — this is an empty
//! tool router holding room for the write surface (send a message, reply,
//! react, …). Each will act as the session's `tag` through the shared Discord
//! client, be gated by `Mode::Full` (add the names to `FULL_ONLY_TOOLS`), and
//! be metered (add `ToolName` variants + per-tool `quota_usage_<tool>` args).

use super::super::PsychologicalOperationsDiscordMcp;

// Empty for now — write tools register here later. The `#[tool_router]` macro
// still emits `pub fn write_tools() -> ToolRouter<Self>` returning an empty
// router, which `new()` combines with the others.
#[rmcp::tool_router(router = write_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {}
