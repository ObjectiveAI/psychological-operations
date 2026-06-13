pub mod destinations;

use destinations::Destination;

pub use psychological_operations_sdk::cli::destinations::DeliverySummary;

/// Drain the delivery queue. The CLI handler wraps this; the runtime
/// calls it directly after a successful score+enqueue cycle.
pub async fn drain_queue(
    db: &crate::db::Db,
    psyop_filter: Option<&str>,
    ctx: &crate::context::Context,
) -> Result<DeliverySummary, crate::error::Error> {
    use crate::psyops::psyop;
    use destinations::{send_one, DeliveryItem, Subject};

    let rows = db.list_pending_deliveries(psyop_filter).await?;
    let mut delivered = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let dest: Destination = match serde_json::from_value(row.target.clone()) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("malformed target: {e}");
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg).await?;
                failed += 1;
                continue;
            }
        };
        let post_ids: Vec<String> = match serde_json::from_value(row.post_ids.clone()) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("malformed post_ids: {e}");
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg).await?;
                failed += 1;
                continue;
            }
        };

        // Load the psyop's current definition by name. If it's been
        // deleted since this row was queued, bump-attempt with a clear
        // message.
        let psyop_obj = match psyop::load(&row.psyop, ctx).await {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("psyop load failed: {e}");
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg).await?;
                failed += 1;
                continue;
            }
        };

        // Build the delivery items from the queued IDs. Only score +
        // handle are recoverable (loaded from the persisted `scores` /
        // `posts` tables); the full post body (text, media, engagement)
        // was dropped after scoring, so destinations only ever see
        // id / handle / score.
        let scored = db.get_scored_handles(&post_ids).await?;
        let items: Vec<DeliveryItem> = post_ids.iter().zip(scored.iter())
            .map(|(id, (score, handle))| DeliveryItem {
                id:     id.clone(),
                handle: handle.clone(),
                score:  *score,
            })
            .collect();
        let subject = Subject::Psyop {
            name:   &row.psyop,
            psyop:  &psyop_obj,
            output: &items,
        };

        match send_one(&dest, &subject, ctx).await {
            Ok(()) => {
                db.delete_delivery(row.id).await?;
                delivered += 1;
            }
            Err(e) => {
                let msg = e.to_string();
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg).await?;
                failed += 1;
            }
        }
    }

    Ok(DeliverySummary { delivered, failed })
}
