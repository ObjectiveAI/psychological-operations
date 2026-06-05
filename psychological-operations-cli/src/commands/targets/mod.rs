//! `targets` subcommand surface.
//!
//! Unified CRUD over destination lists. Each of `get` / `add` /
//! `del` takes a mutually-exclusive 3-way **selector**:
//!
//! - `--global` — top-level `Config::targets`.
//! - `--psyop <name>` — `Config::psyops[name].base.targets`.
//! - `--psyop <name> --commit <sha>` —
//!   `Config::psyops[name].commits[sha].targets`.
//!
//! `--commit` only valid alongside `--psyop`; exactly one of the
//! three forms is required. The `Destination` enum + per-destination
//! `send_one` impls + the `drain_queue` runtime hook all live in
//! `crate::targets` — this file owns the clap surface and the
//! dispatch.

use clap::{Args, Subcommand};
use psychological_operations_sdk::cli::Output;

use crate::config::Config;
use crate::error::Error;
use crate::targets::destinations::Destination;

#[derive(Args)]
#[group(id = "selector", required = true, multiple = false)]
pub struct SelectorArgs {
    /// Operate on the top-level global targets list.
    #[arg(long, group = "selector")]
    global: bool,
    /// Operate on a specific psyop's targets list. Without
    /// `--commit`, this is the psyop's base layer.
    #[arg(long, group = "selector", value_name = "NAME")]
    psyop: Option<String>,
    /// When combined with `--psyop`, narrows to that psyop's
    /// commit-specific overrides under `commits.<SHA>`. Cannot be
    /// used with `--global` or on its own.
    #[arg(long, requires = "psyop", conflicts_with = "global", value_name = "SHA")]
    commit: Option<String>,
}

enum Selector {
    Global,
    PsyopBase   { psyop: String },
    PsyopCommit { psyop: String, commit: String },
}

impl SelectorArgs {
    fn resolve(self) -> Result<Selector, Error> {
        match (self.global, self.psyop, self.commit) {
            (true,  None,    None)    => Ok(Selector::Global),
            (false, Some(p), None)    => Ok(Selector::PsyopBase { psyop: p }),
            (false, Some(p), Some(c)) => Ok(Selector::PsyopCommit { psyop: p, commit: c }),
            _ => Err(Error::Other(
                "exactly one of --global, --psyop, or --psyop+--commit is required".into(),
            )),
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Read the targets list selected by --global / --psyop /
    /// --psyop+--commit. Optional `[index]` narrows to a single
    /// entry.
    Get {
        #[command(flatten)]
        selector: SelectorArgs,
        index: Option<usize>,
    },
    /// Append a target (Destination-shaped JSON) to the selected
    /// list.
    Add {
        #[command(flatten)]
        selector: SelectorArgs,
        json: String,
    },
    /// Remove the entry at `<index>` from the selected list.
    Del {
        #[command(flatten)]
        selector: SelectorArgs,
        index: usize,
    },
    /// Drain the delivery queue: read every queued row, attempt
    /// redelivery, delete on success, bump-attempt on failure.
    /// `--psyop <name>` narrows to that psyop's queue rows. (Not
    /// the same as the CRUD selector — the delivery queue has no
    /// global layer.)
    Deliver {
        #[arg(long)]
        psyop: Option<String>,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                Commands::Get { selector, index } => handle_get(selector.resolve()?, index, cfg),
                Commands::Add { selector, json }  => handle_add(selector.resolve()?, json, cfg),
                Commands::Del { selector, index } => handle_del(selector.resolve()?, index, cfg),
                Commands::Deliver { psyop } => {
                    let db = crate::db::Db::open(cfg)?;
                    let summary = crate::targets::drain_queue(&db, psyop.as_deref(), cfg).await?;
                    Ok(Output::Api(serde_json::to_string(&summary)?))
                }
            }
        }.await;
        crate::output::emit_result(result)
    }
}

fn handle_get(
    sel: Selector,
    index: Option<usize>,
    cfg: &crate::run::Config,
) -> Result<Output, Error> {
    let json_cfg = crate::config::load(cfg);
    let list = list_for(&json_cfg, &sel);
    match index {
        Some(i) => {
            let entry = list.get(i).ok_or_else(|| {
                Error::Other(format!("no target at index {i}"))
            })?;
            Ok(Output::ConfigGet(serde_json::to_string(entry)?))
        }
        None => Ok(Output::ConfigGet(serde_json::to_string(&list)?)),
    }
}

fn handle_add(
    sel: Selector,
    json: String,
    cfg: &crate::run::Config,
) -> Result<Output, Error> {
    let parsed: Destination = serde_json::from_str(&json)?;
    let mut json_cfg = crate::config::load(cfg);
    match sel {
        Selector::Global => json_cfg.targets.push(parsed),
        Selector::PsyopBase { psyop } => {
            json_cfg.psyops.entry(psyop).or_default().base.targets.push(parsed);
        }
        Selector::PsyopCommit { psyop, commit } => {
            json_cfg.psyops
                .entry(psyop).or_default()
                .commits
                .entry(commit).or_default()
                .targets.push(parsed);
        }
    }
    crate::config::save(&json_cfg, cfg)?;
    Ok(Output::ConfigSet)
}

fn handle_del(
    sel: Selector,
    index: usize,
    cfg: &crate::run::Config,
) -> Result<Output, Error> {
    let mut json_cfg = crate::config::load(cfg);
    match sel {
        Selector::Global => {
            if index >= json_cfg.targets.len() {
                return Err(Error::Other(format!("no target at index {index}")));
            }
            json_cfg.targets.remove(index);
        }
        Selector::PsyopBase { psyop } => {
            del_from_psyop(&mut json_cfg, &psyop, None, index)?;
        }
        Selector::PsyopCommit { psyop, commit } => {
            del_from_psyop(&mut json_cfg, &psyop, Some(&commit), index)?;
        }
    }
    crate::config::save(&json_cfg, cfg)?;
    Ok(Output::ConfigSet)
}

/// Remove the entry at `index` from a psyop's targets list (base
/// or a specific commit), then prune empty `commits.<sha>` and
/// empty `psyops.<name>` entries.
fn del_from_psyop(
    json_cfg: &mut Config,
    psyop: &str,
    commit: Option<&str>,
    index: usize,
) -> Result<(), Error> {
    {
        let overrides = json_cfg.psyops.get_mut(psyop).ok_or_else(|| {
            Error::Other(format!("no psyop config entry for \"{psyop}\""))
        })?;
        let list = match commit {
            Some(sha) => &mut overrides
                .commits
                .get_mut(sha)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "no commit override \"{sha}\" for psyop \"{psyop}\""
                    ))
                })?
                .targets,
            None => &mut overrides.base.targets,
        };
        if index >= list.len() {
            return Err(Error::Other(format!("no target at index {index}")));
        }
        list.remove(index);
        if let Some(sha) = commit {
            if overrides.commits.get(sha).is_some_and(|c| c.is_empty()) {
                overrides.commits.remove(sha);
            }
        }
    }
    if json_cfg.psyops.get(psyop).is_some_and(|o| o.is_empty()) {
        json_cfg.psyops.remove(psyop);
    }
    Ok(())
}

fn list_for(cfg: &Config, sel: &Selector) -> Vec<Destination> {
    match sel {
        Selector::Global => cfg.targets.clone(),
        Selector::PsyopBase { psyop } => cfg
            .psyops.get(psyop)
            .map(|o| o.base.targets.clone())
            .unwrap_or_default(),
        Selector::PsyopCommit { psyop, commit } => cfg
            .psyops.get(psyop)
            .and_then(|o| o.commits.get(commit))
            .map(|c| c.targets.clone())
            .unwrap_or_default(),
    }
}
