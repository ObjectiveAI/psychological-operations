//! `targets deliver` arm — drain the delivery queue scoped by the
//! 2-way selector:
//!
//! - `Selector::Global` — drain everything (no narrowing).
//! - `Selector::Psyop { psyop }` — drain every row for that psyop.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;

use super::Selector;

pub(super) async fn run(
    sel: Selector,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let psyop = match sel {
        Selector::Global => None,
        Selector::Psyop { psyop } => Some(psyop),
    };
    let summary = crate::targets::drain_queue(&ctx.db, psyop.as_deref(), ctx).await?;
    Ok(Output::DeliverySummary(summary))
}
