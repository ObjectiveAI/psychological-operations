//! `targets list` arm — paginated read of the targets list in
//! the selected layer.

use psychological_operations_sdk::cli::Output;

use crate::config::Config;
use crate::error::Error;
use crate::targets::destinations::Destination;

use super::Selector;

pub(super) fn run(
    sel: Selector,
    count: Option<usize>,
    offset: Option<usize>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let json_cfg = crate::config::load(&ctx.config);
    let list = list_for(&json_cfg, &sel);
    let start = offset.unwrap_or(0);
    let end = match count {
        Some(c) => start.saturating_add(c).min(list.len()),
        None    => list.len(),
    };
    let page: &[Destination] = if start >= list.len() {
        &[]
    } else {
        &list[start..end]
    };
    Ok(Output::ConfigGet(serde_json::to_string(page)?))
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
