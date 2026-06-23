//! The metered MCP tools — shared by the MCP server (read/write
//! classification + per-tool cost lookup at quota-enforcement time).
//!
//! Only quota-charged tools appear here. The queue tools (`read_queue` /
//! `mark_handled`) are deliberately absent: a tool's absence from this enum is
//! exactly what makes it quota-free — `call_tool` skips enforcement for any
//! name `from_name` doesn't know. The write tools add their variants here when
//! that module is filled.

/// Which budget a tool's cost counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Write` is unused until the write tools land.
pub enum Direction {
    Read,
    Write,
}

/// One metered MCP tool. The string forms (`as_name`) match the
/// `#[tool(name = …)]` registrations exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    ListServers,
    ListChannels,
    ListMessages,
    GetMessage,
    OpenAttachment,
}

impl ToolName {
    /// Every metered tool, in a stable order.
    pub const ALL: [ToolName; 5] = [
        ToolName::ListServers,
        ToolName::ListChannels,
        ToolName::ListMessages,
        ToolName::GetMessage,
        ToolName::OpenAttachment,
    ];

    /// The MCP tool name, matching its `#[tool(name = …)]`.
    pub fn as_name(self) -> &'static str {
        use ToolName::*;
        match self {
            ListServers => "list_servers",
            ListChannels => "list_channels",
            ListMessages => "list_messages",
            GetMessage => "get_message",
            OpenAttachment => "open_attachment",
        }
    }

    /// Parse a tool name back to its [`ToolName`]; `None` for unmetered names
    /// (the quota-free queue tools).
    pub fn from_name(s: &str) -> Option<Self> {
        ToolName::ALL.iter().copied().find(|t| t.as_name() == s)
    }

    /// Which budget this tool's cost counts against. All current tools are
    /// reads.
    pub fn direction(self) -> Direction {
        use ToolName::*;
        match self {
            ListServers | ListChannels | ListMessages | GetMessage | OpenAttachment => {
                Direction::Read
            }
        }
    }
}
