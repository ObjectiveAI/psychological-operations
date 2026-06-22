//! Read tools — Discord GET endpoints. **Not implemented yet** — this is an
//! empty tool router holding room for the read surface (list channel messages,
//! fetch a message, resolve a user, …), built as authenticated calls through
//! the shared Discord client. The shared pagination helpers used by the queue
//! tools live here so they're in one place when read tools arrive.

use super::super::PsychologicalOperationsDiscordMcp;
use super::super::tool_error::ToolError;

// Empty for now — read tools register here later. The `#[tool_router]` macro
// still emits `pub fn read_tools() -> ToolRouter<Self>` returning an empty
// router, which `new()` combines with the others.
#[rmcp::tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsDiscordMcp {}

/// Max window size any paginated read (incl. the queue tools) accepts.
const MAX_COUNT: u32 = 100;

/// Reject a `count` over [`MAX_COUNT`] with an agent-visible message.
pub(super) fn check_count(count: u32) -> Result<(), ToolError> {
    if count > MAX_COUNT {
        return Err(ToolError::agent(format!(
            "count is {count}, over the {MAX_COUNT} max — request {MAX_COUNT} or fewer."
        )));
    }
    Ok(())
}

/// A short "N remaining" note appended to a windowed list result.
pub(super) fn remaining_note(
    total_fetched: usize,
    offset: usize,
    count: usize,
    has_more: bool,
) -> String {
    let remaining = total_fetched.saturating_sub(offset + count);
    format!("{}{remaining} remaining", if has_more { "over " } else { "" })
}
