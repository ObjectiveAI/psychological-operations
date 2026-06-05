//! `targets deliver` arm — drain the delivery queue with the
//! same 3-way selector the CRUD arms use.
//!
//! Selector semantics for a drain:
//!
//! - `Selector::Global` — drain everything (no narrowing).
//! - `Selector::PsyopBase { psyop }` — drain every row for that
//!   psyop, across all its commits.
//! - `Selector::PsyopCommit { psyop, commit }` — drain only rows
//!   for that psyop at that specific commit.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;

use super::Selector;

pub(super) async fn run(
    sel: Selector,
    cfg: &crate::run::Config,
) -> Result<Output, Error> {
    let (psyop, commit) = match sel {
        Selector::Global                        => (None,           None),
        Selector::PsyopBase   { psyop }         => (Some(psyop),    None),
        Selector::PsyopCommit { psyop, commit } => (Some(psyop),    Some(commit)),
    };
    let db = crate::db::Db::open(cfg)?;
    let summary = crate::targets::drain_queue(
        &db, psyop.as_deref(), commit.as_deref(), cfg,
    ).await?;
    Ok(Output::Api(serde_json::to_string(&summary)?))
}
