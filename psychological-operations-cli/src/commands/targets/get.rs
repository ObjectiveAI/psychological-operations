//! `targets get` arm.

use psychological_operations_sdk::cli::Output;

use crate::config::Config;
use crate::error::Error;
use crate::targets::destinations::Destination;

use super::Selector;

pub(super) fn run(
    sel: Selector,
    index: Option<usize>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let json_cfg = crate::config::load(&ctx.config);
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
