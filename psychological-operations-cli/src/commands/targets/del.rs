//! `targets del` arm.

use psychological_operations_sdk::cli::Output;

use crate::config::Config;
use crate::error::Error;

use super::Selector;

pub(super) fn run(
    sel: Selector,
    index: usize,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let mut json_cfg = crate::config::load(&ctx.config);
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
    crate::config::save(&json_cfg, &ctx.config)?;
    Ok(Output::Ok)
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
