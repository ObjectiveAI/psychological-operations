//! The metered MCP tools — shared by the MCP server (read/write
//! classification + per-tool cost lookup at quota-enforcement time).
//!
//! Only quota-charged tools appear here. A tool's absence from this enum is
//! exactly what makes it quota-free — `call_tool` skips enforcement for any
//! name `from_name` doesn't know. (Every Twitch tool is metered today, so this
//! enum covers them all.)

/// Which budget a tool's cost counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

/// One metered MCP tool. The string forms (`as_name`) match the
/// `#[tool(name = …)]` registrations exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    Whoami,
    ListChannels,
    ListMessages,
    SendMessage,
}

impl ToolName {
    /// Every metered tool, in a stable order.
    pub const ALL: [ToolName; 4] = [
        ToolName::Whoami,
        ToolName::ListChannels,
        ToolName::ListMessages,
        ToolName::SendMessage,
    ];

    /// The MCP tool name, matching its `#[tool(name = …)]`.
    pub fn as_name(self) -> &'static str {
        use ToolName::*;
        match self {
            Whoami => "whoami",
            ListChannels => "list_channels",
            ListMessages => "list_messages",
            SendMessage => "send_message",
        }
    }

    /// Parse a tool name back to its [`ToolName`]; `None` for unmetered names.
    pub fn from_name(s: &str) -> Option<Self> {
        ToolName::ALL.iter().copied().find(|t| t.as_name() == s)
    }

    /// Which budget this tool's cost counts against.
    pub fn direction(self) -> Direction {
        use ToolName::*;
        match self {
            Whoami | ListChannels | ListMessages => Direction::Read,
            SendMessage => Direction::Write,
        }
    }
}
