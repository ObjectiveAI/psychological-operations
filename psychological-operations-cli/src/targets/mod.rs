pub mod destinations;

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use destinations::Destination;

pub use psychological_operations_sdk::cli::destinations::DeliverySummary;

/// Outcome of one delivery, surfaced from a [`deliver_one`] future to the
/// driver loop. Emission stays in the driver (not the futures) so stdout
/// lines remain serialized and ordered by completion.
enum Outcome {
    Delivered,
    Failed { delivery_id: i64, reason: String },
}

/// Drain the delivery queue. The CLI handler wraps this; the runtime
/// calls it directly after a successful score+enqueue cycle.
///
/// Every pending row is delivered **concurrently** — one [`deliver_one`]
/// future each — and consumed in **completion order** via
/// [`FuturesUnordered`], emitting + tallying as each finishes. A
/// recoverable per-delivery problem resolves to [`Outcome::Failed`]
/// (counted, attempt bumped); a hard persistence failure propagates and
/// aborts the drain.
pub async fn drain_queue(
    db: &crate::db::Db,
    psyop_filter: Option<&str>,
    ctx: &crate::context::Context,
) -> Result<DeliverySummary, crate::error::Error> {
    let rows = db.list_pending_deliveries(psyop_filter).await?;

    let mut inflight: FuturesUnordered<_> = rows
        .into_iter()
        .map(|row| deliver_one(db, ctx, row))
        .collect();

    let mut delivered = 0usize;
    let mut failed = 0usize;
    while let Some(outcome) = inflight.next().await {
        match outcome? {
            Outcome::Delivered => delivered += 1,
            Outcome::Failed { delivery_id, reason } => {
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed {
                    delivery_id,
                    reason,
                })
                .emit();
                failed += 1;
            }
        }
    }

    Ok(DeliverySummary { delivered, failed })
}

/// Deliver one queued row: parse it, load the psyop, build the items, and
/// send. Recoverable problems (malformed row, missing psyop, send error)
/// bump the row's attempt counter and resolve to [`Outcome::Failed`]; a
/// hard persistence failure propagates as `Err` and aborts the drain.
async fn deliver_one(
    db: &crate::db::Db,
    ctx: &crate::context::Context,
    row: crate::db::QueuedDelivery,
) -> Result<Outcome, crate::error::Error> {
    use crate::psyops::psyop;
    use destinations::{DeliveryItem, Subject, send_one};

    let dest: Destination = match serde_json::from_value(row.target.clone()) {
        Ok(d) => d,
        Err(e) => return fail(db, row.id, format!("malformed target: {e}")).await,
    };
    let post_ids: Vec<String> = match serde_json::from_value(row.post_ids.clone()) {
        Ok(v) => v,
        Err(e) => return fail(db, row.id, format!("malformed post_ids: {e}")).await,
    };

    // Load the psyop's current definition by name. If it's been deleted
    // since this row was queued, bump-attempt with a clear message.
    let psyop_obj = match psyop::load(&row.psyop, ctx).await {
        Ok(p) => p,
        Err(e) => return fail(db, row.id, format!("psyop load failed: {e}")).await,
    };

    // Build the delivery items from the queued IDs. Only score + handle
    // are recoverable (loaded from the persisted `scores` / `posts`
    // tables); the full post body (text, media, engagement) was dropped
    // after scoring, so destinations only ever see id / handle / score.
    let scored = db.get_scored_handles(&post_ids).await?;
    let items: Vec<DeliveryItem> = post_ids
        .iter()
        .zip(scored.iter())
        .map(|(id, (score, handle))| DeliveryItem {
            id: id.clone(),
            handle: handle.clone(),
            score: *score,
        })
        .collect();
    let subject = Subject::Psyop {
        name: &row.psyop,
        psyop: &psyop_obj,
        output: &items,
    };

    match send_one(&dest, &subject, ctx).await {
        Ok(()) => {
            db.delete_delivery(row.id).await?;
            Ok(Outcome::Delivered)
        }
        Err(e) => fail(db, row.id, e.to_string()).await,
    }
}

/// Record a recoverable delivery failure: bump the row's attempt counter
/// (a bump failure is itself a hard error → `Err`) and resolve to
/// [`Outcome::Failed`] for the driver to emit.
async fn fail(
    db: &crate::db::Db,
    delivery_id: i64,
    reason: String,
) -> Result<Outcome, crate::error::Error> {
    db.bump_delivery_attempt(delivery_id, &reason).await?;
    Ok(Outcome::Failed { delivery_id, reason })
}
