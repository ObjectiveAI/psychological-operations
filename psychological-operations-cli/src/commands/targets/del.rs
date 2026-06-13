//! `targets del` arm.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;

use super::Selector;

pub(super) async fn run(
    sel: Selector,
    index: usize,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    match sel {
        Selector::Global => {
            let mut list = crate::config::global_targets(ctx).await?;
            remove_at(&mut list, index)?;
            crate::config::set_global_targets(ctx, &list).await?;
        }
        Selector::Psyop { psyop } => {
            let mut list = crate::config::psyop_targets(ctx, &psyop).await?;
            remove_at(&mut list, index)?;
            crate::config::set_psyop_targets(ctx, &psyop, &list).await?;
        }
    }
    Ok(Output::Ok)
}

fn remove_at<T>(list: &mut Vec<T>, index: usize) -> Result<(), Error> {
    if index >= list.len() {
        return Err(Error::Other(format!("no target at index {index}")));
    }
    list.remove(index);
    Ok(())
}
