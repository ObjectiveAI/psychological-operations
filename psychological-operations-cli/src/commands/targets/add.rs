//! `targets add` arm.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::targets::destinations::Destination;

use super::Selector;

pub(super) async fn run(
    sel: Selector,
    json: String,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let parsed: Destination = serde_json::from_str(&json)?;
    match sel {
        Selector::Global => {
            let mut list = crate::config::global_targets(ctx).await?;
            list.push(parsed);
            crate::config::set_global_targets(ctx, &list).await?;
        }
        Selector::Psyop { psyop } => {
            let mut list = crate::config::psyop_targets(ctx, &psyop).await?;
            list.push(parsed);
            crate::config::set_psyop_targets(ctx, &psyop, &list).await?;
        }
    }
    Ok(Output::Ok)
}
