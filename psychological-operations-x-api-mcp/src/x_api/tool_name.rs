//! The metered MCP tools — shared by the MCP server (read/write
//! classification + per-tool cost lookup at quota-enforcement time) and
//! the CLI's `agents quota tool` command (as a clap `ValueEnum`).
//!
//! Only quota-charged tools appear here. Deliberately absent are the
//! two quota-free tools: `read_queue` and `mark_handled`. A tool's
//! absence from this enum is exactly what makes it quota-free —
//! `call_tool` skips enforcement for any name `from_name` doesn't know.
//! Each tool present here is intrinsically a read XOR a write.

/// Which budget a tool's cost counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

/// One metered MCP tool. The string forms (`as_name`) match the
/// `#[tool(name = …)]` registrations exactly — a unit test asserts the
/// set round-trips against the live tool router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    GetReplies,
    GetBio,
    GetProfilePicture,
    GetTweet,
    OpenAttachment,
    RunQuery,
    Whoami,
    GetBookmarks,
    ListFollowing,
    ListFollowers,
    Post,
    Reply,
    Quote,
    Like,
    Retweet,
    Bookmark,
    Follow,
    Unfollow,
}

impl ToolName {
    /// Every metered tool, in a stable order (clap value list + tests).
    pub const ALL: [ToolName; 18] = [
        ToolName::GetReplies,
        ToolName::GetBio,
        ToolName::GetProfilePicture,
        ToolName::GetTweet,
        ToolName::OpenAttachment,
        ToolName::RunQuery,
        ToolName::Whoami,
        ToolName::GetBookmarks,
        ToolName::ListFollowing,
        ToolName::ListFollowers,
        ToolName::Post,
        ToolName::Reply,
        ToolName::Quote,
        ToolName::Like,
        ToolName::Retweet,
        ToolName::Bookmark,
        ToolName::Follow,
        ToolName::Unfollow,
    ];

    /// The MCP tool name, matching its `#[tool(name = …)]`.
    pub fn as_name(self) -> &'static str {
        use ToolName::*;
        match self {
            GetReplies => "get_replies",
            GetBio => "get_bio",
            GetProfilePicture => "get_profile_picture",
            GetTweet => "get_tweet",
            OpenAttachment => "open_attachment",
            RunQuery => "run_query",
            Whoami => "whoami",
            GetBookmarks => "get_bookmarks",
            ListFollowing => "list_following",
            ListFollowers => "list_followers",
            Post => "post",
            Reply => "reply",
            Quote => "quote",
            Like => "like",
            Retweet => "retweet",
            Bookmark => "bookmark",
            Follow => "follow",
            Unfollow => "unfollow",
        }
    }

    /// Parse a tool name back to its [`ToolName`]; `None` for unmetered
    /// names (e.g. `list_accounts`).
    pub fn from_name(s: &str) -> Option<Self> {
        ToolName::ALL.iter().copied().find(|t| t.as_name() == s)
    }

    /// Which budget this tool's cost counts against.
    pub fn direction(self) -> Direction {
        use ToolName::*;
        match self {
            GetReplies | GetBio | GetProfilePicture | GetTweet | OpenAttachment | RunQuery
            | Whoami | GetBookmarks | ListFollowing | ListFollowers => Direction::Read,
            Post | Reply | Quote | Like | Retweet | Bookmark | Follow | Unfollow => {
                Direction::Write
            }
        }
    }
}

// Hand-rolled so the clap value spelling IS `as_name()` — one source of
// truth shared with the MCP's `request.name` matching.
impl clap::ValueEnum for ToolName {
    fn value_variants<'a>() -> &'a [Self] {
        &ToolName::ALL
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.as_name()))
    }
}
