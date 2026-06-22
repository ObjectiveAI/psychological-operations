//! `agents daemon discord hooks {insert,list,delete}` — manage the Python
//! hooks the Discord gateway daemon runs for an agent. A hook has a name +
//! description + Python source; the daemon runs it for every gateway event.

use clap::{Args, Subcommand};

pub mod delete;
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

#[derive(Subcommand)]
pub enum Commands {
    /// Add a named hook for an agent. Replacing an existing hook of the same
    /// name requires `--overwrite`.
    Insert {
        /// Agent tag the hook belongs to.
        #[arg(long)]
        agent_tag: String,
        /// Hook name (unique per agent).
        #[arg(long)]
        name: String,
        /// Human-readable description of what the hook does.
        #[arg(long)]
        description: String,
        /// Required to replace a hook that already exists with this name.
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        source: PythonSource,
    },
    /// List an agent's hooks (name + description).
    List {
        /// Agent tag whose hooks to list.
        #[arg(long)]
        agent_tag: String,
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
            Commands::Insert {
                agent_tag,
                name,
                description,
                overwrite,
                source,
            } => insert::run(&agent_tag, &name, &description, overwrite, source, ctx).await,
            Commands::List { agent_tag } => list::run(&agent_tag, ctx).await,
            Commands::Delete { agent_tag, name } => delete::run(&agent_tag, &name, ctx).await,
        }
    }
}
