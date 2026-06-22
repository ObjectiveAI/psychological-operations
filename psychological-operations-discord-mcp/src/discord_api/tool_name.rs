//! The metered MCP tools — shared by the MCP server (read/write
//! classification + per-tool cost lookup at quota-enforcement time).
//!
//! Only quota-charged tools appear here. The queue tools (`read_queue` /
//! `mark_handled`) are deliberately absent: a tool's absence from this enum is
//! exactly what makes it quota-free — `call_tool` skips enforcement for any
//! name `from_name` doesn't know.
//!
//! **There are no metered Discord tools yet** — `ToolName` is intentionally
//! empty. The read/write tools (and their `ToolName` variants + per-tool
//! `quota_usage_<tool>` arguments) get added here when those modules are
//! filled. The quota machinery in `mod.rs` is wired against this enum so they
//! become drop-in.

/// Which budget a tool's cost counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

/// One metered MCP tool. Empty for now (no metered Discord tools); the
/// read/write tools add variants here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {}

impl ToolName {
    /// Every metered tool, in a stable order. Empty until read/write tools land.
    pub const ALL: [ToolName; 0] = [];

    /// The MCP tool name, matching its `#[tool(name = …)]`. Unreachable while
    /// the enum is empty.
    pub fn as_name(self) -> &'static str {
        match self {}
    }

    /// Parse a tool name back to its [`ToolName`]; always `None` while there
    /// are no metered tools.
    pub fn from_name(_s: &str) -> Option<Self> {
        None
    }

    /// Which budget this tool's cost counts against. Unreachable while empty.
    pub fn direction(self) -> Direction {
        match self {}
    }
}
