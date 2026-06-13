//! `targets list` arm — paginated read of the selected targets list.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::targets::destinations::Destination;

use super::Selector;

pub(super) async fn run(
    sel: Selector,
    count: Option<usize>,
    offset: Option<usize>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let list: Vec<Destination> = match sel {
        Selector::Global => crate::config::global_targets(ctx).await?,
        Selector::Psyop { psyop } => crate::config::psyop_targets(ctx, &psyop).await?,
    };
    let start = offset.unwrap_or(0);
    let end = match count {
        Some(c) => start.saturating_add(c).min(list.len()),
        None => list.len(),
    };
    let page: &[Destination] = if start >= list.len() {
        &[]
    } else {
        &list[start..end]
    };
    Ok(Output::DestinationList(page.to_vec()))
}
