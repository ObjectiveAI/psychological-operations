//! `agents daemon discord hooks {insert,list,delete}` — manage the hooks the
//! Discord gateway daemon runs for an agent. A hook has a name + description +
//! a typed definition: `python` (run for every gateway event) or one of the
//! declarative triggers (`mention` / `reply` / `dm`) the daemon evaluates
//! against incoming messages, enqueueing on a match.

use clap::{Args, Subcommand};

pub mod delete;
pub mod get;
pub mod insert;
pub mod list;

/// Where the hook's Python source comes from — inline or a file. Exactly one.
#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct PythonSource {
    /// Inline Python source for the hook.
    #[arg(long)]
    python_inline: Option<String>,
    /// Path to a file containing the hook's Python source.
    #[arg(long)]
    python_file: Option<std::path::PathBuf>,
}

impl PythonSource {
    /// Resolve the source to its Python string (reads the file if given).
    pub fn resolve(self) -> Result<String, crate::error::Error> {
        if let Some(code) = self.python_inline {
            Ok(code)
        } else if let Some(path) = self.python_file {
            std::fs::read_to_string(&path).map_err(|e| {
                crate::error::Error::Other(format!("read {}: {e}", path.display()))
            })
        } else {
            unreachable!("clap group ensures one is set")
        }
    }
}

/// Fields shared by every hook type at insert time.
#[derive(Args)]
pub struct CommonArgs {
    /// Agent tag the hook belongs to.
    #[arg(long)]
    pub agent_tag: String,
    /// Hook name (unique per agent).
    #[arg(long)]
    pub name: String,
    /// Human-readable description of what the hook does.
    #[arg(long)]
    pub description: String,
    /// Required to replace a hook that already exists with this name.
    #[arg(long)]
    pub overwrite: bool,
}

/// One hook type to insert. The declarative types (`mention`/`reply`/`dm`) take
/// an optional `--user-id` (defaults to the agent's own bot user) and a
/// required `--message` (the note delivered to the agent on a match).
#[derive(Subcommand)]
pub enum InsertHook {
    /// Python run for every gateway event, with the raw event JSON as input.
    Python {
        #[command(flatten)]
        common: CommonArgs,
        #[command(flatten)]
        source: PythonSource,
    },
    /// Enqueue when a message `@everyone`s, mentions the user, or mentions a
    /// role the user holds.
    Mention {
        #[command(flatten)]
        common: CommonArgs,
        /// Discord user id to watch (default: the agent's own bot user).
        #[arg(long)]
        user_id: Option<String>,
        /// Note delivered to the agent on a match.
        #[arg(long)]
        message: String,
    },
    /// Enqueue when a message replies to one authored by the user.
    Reply {
        #[command(flatten)]
        common: CommonArgs,
        /// Discord user id to watch (default: the agent's own bot user).
        #[arg(long)]
        user_id: Option<String>,
        /// Note delivered to the agent on a match.
        #[arg(long)]
        message: String,
    },
    /// Enqueue on any incoming DM.
    Dm {
        #[command(flatten)]
        common: CommonArgs,
        /// Discord user id whose own messages are excluded (default: the
        /// agent's own bot user, i.e. incoming DMs only).
        #[arg(long)]
        user_id: Option<String>,
        /// Note delivered to the agent on a match.
        #[arg(long)]
        message: String,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Add a named hook for an agent. Replacing an existing hook of the same
    /// name requires `--overwrite`.
    Insert {
        #[command(subcommand)]
        hook: InsertHook,
    },
    /// List an agent's hooks (name + type + description).
    List {
        /// Agent tag whose hooks to list.
        #[arg(long)]
        agent_tag: String,
    },
    /// Show one hook's full typed definition.
    Get {
        /// Agent tag the hook belongs to.
        #[arg(long)]
        agent_tag: String,
        /// Name of the hook to show.
        #[arg(long)]
        name: String,
    },
    /// Delete a named hook from an agent.
    Delete {
        /// Agent tag the hook belongs to.
        #[arg(long)]
        agent_tag: String,
        /// Name of the hook to delete.
        #[arg(long)]
        name: String,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Insert { hook } => insert::run(hook, ctx).await,
            Commands::List { agent_tag } => list::run(&agent_tag, ctx).await,
            Commands::Get { agent_tag, name } => get::run(&agent_tag, &name, ctx).await,
            Commands::Delete { agent_tag, name } => delete::run(&agent_tag, &name, ctx).await,
        }
    }
}
