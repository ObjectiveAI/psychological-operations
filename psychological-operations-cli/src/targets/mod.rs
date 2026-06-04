pub mod destinations;

use destinations::Destination;

#[derive(serde::Serialize)]
pub struct DeliverySummary {
    pub delivered: usize,
    pub failed:    usize,
}

/// Drain the delivery queue. The CLI handler wraps this; the runtime
/// calls it directly after a successful score+enqueue cycle.
pub async fn drain_queue(
    db: &crate::db::Db,
    psyop_filter: Option<&str>,
    cfg: &crate::run::Config,
) -> Result<DeliverySummary, crate::error::Error> {
    use crate::db::{MediaUrl, Post};
    use crate::psyops::psyop;
    use crate::score::ScoredPost;
    use destinations::{send_one, Subject};

    let rows = db.list_pending_deliveries(psyop_filter)?;
    let mut delivered = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let dest: Destination = match serde_json::from_str(&row.target_json) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("malformed target_json: {e}");
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };
        let post_ids: Vec<String> = match serde_json::from_str(&row.post_ids_json) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("malformed post_ids_json: {e}");
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };

        // Load the psyop as it existed at the queued commit_sha
        // (git tree blob, not working tree). If the repo / commit /
        // file is missing, bump-attempt with a clear message.
        let psyop_obj = match psyop::load(&row.psyop, Some(&row.psyop_commit_sha), cfg) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("psyop load at {} failed: {e}", row.psyop_commit_sha);
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };

        // Synthesize stub ScoredPosts from the queued IDs. Score +
        // handle are loaded back from the persisted `scores` and
        // `posts` tables so stdout delivery shows real numbers and
        // well-formed `https://x.com/<handle>/status/<id>` URLs; the
        // rest of the Post (text, media, …) stays empty — `contents`
        // is dropped after scoring, and X delivery only reads
        // post.id.
        let scored = db.get_scored_handles(&post_ids)?;
        let stubs: Vec<ScoredPost> = post_ids.iter().zip(scored.iter())
            .map(|(id, (score, handle))| ScoredPost {
                post: Post {
                    id: id.clone(),
                    handle: handle.clone(),
                    text: String::new(),
                    images: Vec::<MediaUrl>::new(),
                    videos: Vec::<MediaUrl>::new(),
                    created: String::new(),
                    likes: 0, retweets: 0, replies: 0, impressions: 0,
                },
                score: *score,
            })
            .collect();
        let stub_refs: Vec<&ScoredPost> = stubs.iter().collect();
        let subject = Subject::Psyop {
            name:   &row.psyop,
            psyop:  &psyop_obj,
            output: &stub_refs,
        };

        match send_one(&dest, &subject, cfg).await {
            Ok(()) => {
                db.delete_delivery(row.id)?;
                delivered += 1;
            }
            Err(e) => {
                let msg = e.to_string();
                crate::output::OutputResult::from(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() }).emit();
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
            }
        }
    }

    Ok(DeliverySummary { delivered, failed })
}
