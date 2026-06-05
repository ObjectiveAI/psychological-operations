//! `targets add` arm.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::targets::destinations::Destination;

use super::Selector;

pub(super) fn run(
    sel: Selector,
    json: String,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let parsed: Destination = serde_json::from_str(&json)?;
    let mut json_cfg = crate::config::load(&ctx.config);
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
    crate::config::save(&json_cfg, &ctx.config)?;
    Ok(Output::ConfigSet)
}
