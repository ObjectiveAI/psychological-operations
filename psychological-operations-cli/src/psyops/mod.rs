pub mod browse;
pub mod targets;
pub mod run;

pub mod psyop;
pub mod query;
pub mod for_you;
pub mod sort_by;
pub mod filter;
pub mod stage;

pub use psyop::*;
pub use query::*;
pub use for_you::*;
pub use sort_by::*;
pub use filter::*;
pub use stage::*;

use clap::{Args, Subcommand};
use serde::Serialize;

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
        /// Pass-through to `objectiveai` for deterministic mock
        /// outputs. Used by integration tests; optional otherwise.
        #[arg(long)]
        seed: Option<i64>,
    },
    /// Open chromium for each psyop in turn so the operator can
    /// scroll x.com / capture tweets via the extension. Blocks on
    /// each chromium's exit before opening the next. With
    /// `--name <X>` opens just that one psyop's chromium.
    Browse {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, requires = "name")]
        commit: Option<String>,
    },
    /// Manage per-psyop target destinations.
    Targets {
        #[command(subcommand)]
        command: targets::Commands,
    },
    /// Authorize the per-psyop X account via OAuth 2.0 (PKCE) so the
    /// X target can like / retweet on its behalf. Opens chromium on
    /// the per-psyop profile (which must already be signed into X)
    /// and persists tokens to ~/.psychological-operations/tokens/<name>.json.
    /// One-time per psyop — refresh tokens are used silently after that.
    OAuth {
        name: String,
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
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::List { enabled, disabled } => list(enabled, disabled, cfg),
            Commands::Get { name } => get(&name, cfg),
            Commands::Enable { name, commit } => set_disabled(&name, commit.as_deref(), false, cfg),
            Commands::Disable { name, commit } => set_disabled(&name, commit.as_deref(), true, cfg),
            Commands::Publish { args } => publish(args, cfg),
            Commands::Run { name, commit, seed } => run::run_all(name.as_deref(), commit.as_deref(), seed, cfg).await,
            Commands::Browse { name, commit } => browse::run(name.as_deref(), commit.as_deref(), cfg).await,
            Commands::Targets { command } => command.handle(cfg),
            Commands::OAuth { name } => crate::oauth::setup::run(&name, cfg).await,
        }
    }
}

fn list(enabled: bool, disabled: bool, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
    let json_cfg = crate::config::load(cfg);
    let dir = crate::config::psyops_dir(cfg);
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
            let is_enabled = !json_cfg.psyops.get(&name)
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

fn get(name: &str, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
    let psyop = self::psyop::load(name, None, cfg)?;
    Ok(crate::Output::ConfigGet(serde_json::to_string(&psyop)?))
}

fn set_disabled(name: &str, commit: Option<&str>, value: bool, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
    let mut json_cfg = crate::config::load(cfg);
    {
        let overrides = json_cfg.psyops.entry(name.to_string()).or_default();
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
    if json_cfg.psyops.get(name).is_some_and(|o| o.is_empty()) {
        json_cfg.psyops.remove(name);
    }
    crate::config::save(&json_cfg, cfg)?;
    Ok(crate::Output::ConfigSet)
}

fn publish(args: PublishArgs, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
    let psyop: PsyOp = if let Some(inline) = args.source.psyop_inline {
        serde_json::from_str(&inline)?
    } else if let Some(path) = args.source.psyop_file {
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data)?
    } else {
        unreachable!("clap group ensures one is set")
    };
    psyop.validate()?;
    let dir = crate::config::psyops_dir(cfg).join(&args.name);
    let json = serde_json::to_string_pretty(&psyop)? + "\n";
    let sha = crate::publish::publish_file(&dir, "psyop.json", &json, &args.message, cfg)?;
    Ok(crate::Output::Api(sha))
}
