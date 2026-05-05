pub mod notifications;
pub mod run;

pub mod psyop;
pub mod query;
pub mod for_you;
pub mod sort_by;

pub use psyop::*;
pub use query::*;
pub use for_you::*;
pub use sort_by::*;

use clap::{Args, Subcommand};
use serde::Serialize;

use self::psyop::PsyOp;

#[derive(Subcommand)]
pub enum Commands {
    /// List all psyops on disk. `enabled` reflects the resolved state at
    /// each psyop's current commit. `--enabled` and `--disabled` are
    /// mutually exclusive filters.
    List {
        #[arg(long, conflicts_with = "disabled")]
        enabled: bool,
        #[arg(long)]
        disabled: bool,
    },
    /// Print the on-disk JSON definition of a psyop.
    Get {
        name: String,
    },
    /// Mark a psyop as enabled. With `--commit <sha>` only affects that
    /// commit; otherwise updates the base flag.
    Enable {
        name: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Mark a psyop as disabled. With `--commit <sha>` only affects that
    /// commit; otherwise updates the base flag.
    Disable {
        name: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Publish a psyop definition (writes psyop.json + commits in its repo).
    Publish {
        #[command(flatten)]
        args: PublishArgs,
    },
    /// Run enabled psyops in rounds: each round runs all psyops that have
    /// enough data concurrently; later rounds pick up psyops whose inputs
    /// depend on earlier rounds' scores. With no flags, runs the full set.
    /// `--name X` narrows the run to one psyop; `--commit Y` additionally
    /// requires the psyop's HEAD to match Y. `--commit` without `--name`
    /// is rejected.
    Run {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, requires = "name")]
        commit: Option<String>,
    },
    /// Manage per-psyop notification destinations.
    Notifications {
        #[command(subcommand)]
        command: notifications::Commands,
    },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct PsyopSource {
    /// Inline JSON psyop definition
    #[arg(long)]
    psyop_inline: Option<String>,
    /// Path to a JSON file containing the psyop definition
    #[arg(long)]
    psyop_file: Option<std::path::PathBuf>,
}

#[derive(Args)]
pub struct PublishArgs {
    /// Psyop name
    #[arg(long)]
    pub name: String,
    #[command(flatten)]
    pub source: PsyopSource,
    /// Commit message
    #[arg(long)]
    pub message: String,
}

#[derive(Serialize)]
struct PsyopEntry {
    name: String,
    enabled: bool,
    commit_sha: String,
}

impl Commands {
    pub async fn handle(self) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::List { enabled, disabled } => list(enabled, disabled),
            Commands::Get { name } => get(&name),
            Commands::Enable { name, commit } => set_disabled(&name, commit.as_deref(), false),
            Commands::Disable { name, commit } => set_disabled(&name, commit.as_deref(), true),
            Commands::Publish { args } => publish(args),
            Commands::Run { name, commit } => run::run_all(name.as_deref(), commit.as_deref()).await,
            Commands::Notifications { command } => command.handle(),
        }
    }
}

fn list(enabled: bool, disabled: bool) -> Result<crate::Output, crate::error::Error> {
    let cfg = crate::config::load();
    let dir = crate::config::psyops_dir();
    let mut entries: Vec<PsyopEntry> = Vec::new();
    if dir.exists() {
        for ent in std::fs::read_dir(&dir)? {
            let ent = ent?;
            let path = ent.path();
            if !path.is_dir()
                || !path.join("psyop.json").exists()
                || !path.join(".git").exists()
            {
                continue;
            }
            let Some(name) = ent.file_name().to_str().map(|s| s.to_string()) else { continue };
            let commit_sha = (|| -> Result<String, git2::Error> {
                let repo = git2::Repository::open(&path)?;
                let head = repo.head()?.peel_to_commit()?;
                Ok(head.id().to_string())
            })().unwrap_or_default();
            let is_enabled = !cfg.psyops.get(&name)
                .map(|o| o.disabled_for(&commit_sha))
                .unwrap_or(false);
            if enabled && !is_enabled { continue; }
            if disabled && is_enabled { continue; }
            entries.push(PsyopEntry { name, enabled: is_enabled, commit_sha });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(crate::Output::ConfigGet(serde_json::to_string(&entries)?))
}

fn get(name: &str) -> Result<crate::Output, crate::error::Error> {
    let psyop = crate::psyop::load(name)?;
    Ok(crate::Output::ConfigGet(serde_json::to_string(&psyop)?))
}

fn set_disabled(name: &str, commit: Option<&str>, value: bool) -> Result<crate::Output, crate::error::Error> {
    let mut cfg = crate::config::load();
    {
        let overrides = cfg.psyops.entry(name.to_string()).or_default();
        match commit {
            Some(sha) => {
                overrides.commits.entry(sha.to_string()).or_default().disabled = Some(value);
                if overrides.commits.get(sha).is_some_and(|c| c.is_empty()) {
                    overrides.commits.remove(sha);
                }
            }
            None => {
                overrides.base.disabled = Some(value);
            }
        }
    }
    if cfg.psyops.get(name).is_some_and(|o| o.is_empty()) {
        cfg.psyops.remove(name);
    }
    crate::config::save(&cfg)?;
    Ok(crate::Output::ConfigSet)
}

fn publish(args: PublishArgs) -> Result<crate::Output, crate::error::Error> {
    let psyop: PsyOp = if let Some(inline) = args.source.psyop_inline {
        serde_json::from_str(&inline)?
    } else if let Some(path) = args.source.psyop_file {
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data)?
    } else {
        unreachable!("clap group ensures one is set")
    };
    psyop.validate()?;
    let dir = crate::config::psyops_dir().join(&args.name);
    let json = serde_json::to_string_pretty(&psyop)? + "\n";
    let sha = crate::publish::publish_file(&dir, "psyop.json", &json, &args.message)?;
    Ok(crate::Output::Api(sha))
}
