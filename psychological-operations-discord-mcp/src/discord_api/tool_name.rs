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
pub enum Direction {
    Read,
    Write,
}

/// One metered MCP tool. The string forms (`as_name`) match the
/// `#[tool(name = …)]` registrations exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    Whoami,
    ListServers,
    ListChannels,
    ListUsers,
    ListRoleMembers,
    ListMessages,
    GetMessage,
    GetUser,
    GetProfilePicture,
    GetRole,
    ListAvailableReactions,
    GetMessageReactionsByUser,
    OpenAttachment,
    SendMessage,
    SendDirectMessage,
}

impl ToolName {
    /// Every metered tool, in a stable order.
    pub const ALL: [ToolName; 15] = [
        ToolName::Whoami,
        ToolName::ListServers,
        ToolName::ListChannels,
        ToolName::ListUsers,
        ToolName::ListRoleMembers,
        ToolName::ListMessages,
        ToolName::GetMessage,
        ToolName::GetUser,
        ToolName::GetProfilePicture,
        ToolName::GetRole,
        ToolName::ListAvailableReactions,
        ToolName::GetMessageReactionsByUser,
        ToolName::OpenAttachment,
        ToolName::SendMessage,
        ToolName::SendDirectMessage,
    ];

    /// The MCP tool name, matching its `#[tool(name = …)]`.
    pub fn as_name(self) -> &'static str {
        use ToolName::*;
        match self {
            Whoami => "whoami",
            ListServers => "list_servers",
            ListChannels => "list_channels",
            ListUsers => "list_users",
            ListRoleMembers => "list_role_members",
            ListMessages => "list_messages",
            GetMessage => "get_message",
            GetUser => "get_user",
            GetProfilePicture => "get_profile_picture",
            GetRole => "get_role",
            ListAvailableReactions => "list_available_reactions",
            GetMessageReactionsByUser => "get_message_reactions_by_user",
            OpenAttachment => "open_attachment",
            SendMessage => "send_message",
            SendDirectMessage => "send_direct_message",
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
            Whoami | ListServers | ListChannels | ListUsers | ListRoleMembers | ListMessages
            | GetMessage | GetUser | GetProfilePicture | GetRole | ListAvailableReactions
            | GetMessageReactionsByUser | OpenAttachment => Direction::Read,
            SendMessage | SendDirectMessage => Direction::Write,
        }
    }
}
