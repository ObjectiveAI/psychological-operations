pub mod destinations;

use clap::Subcommand;

use destinations::Destination;

#[derive(Subcommand)]
pub enum Commands {
    /// Get all global targets, or one by index
    Get {
        index: Option<usize>,
    },
    /// Add a global target (JSON string)
    Add {
        json: String,
    },
    /// Remove a global target by index
    Del {
        index: usize,
    },
    /// Drain the delivery queue: read every queued row, attempt
    /// redelivery, delete on success, bump-attempt on failure.
    /// `--psyop <name>` narrows to that psyop's queue rows.
    Deliver {
        #[arg(long)]
        psyop: Option<String>,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::Get { index } => {
                let json_cfg = crate::config::load(cfg);
                match index {
                    Some(i) => {
                        let entry = json_cfg.targets.get(i)
                            .ok_or_else(|| crate::error::Error::Other(format!("no target at index {i}")))?;
                        Ok(crate::Output::ConfigGet(serde_json::to_string(entry)?))
                    }
                    None => Ok(crate::Output::ConfigGet(serde_json::to_string(&json_cfg.targets)?)),
                }
            }
            Commands::Add { json } => {
                let parsed: Destination = serde_json::from_str(&json)?;
                let mut json_cfg = crate::config::load(cfg);
                json_cfg.targets.push(parsed);
                crate::config::save(&json_cfg, cfg)?;
                Ok(crate::Output::ConfigSet)
            }
            Commands::Del { index } => {
                let mut json_cfg = crate::config::load(cfg);
                if index >= json_cfg.targets.len() {
                    return Err(crate::error::Error::Other(format!("no target at index {index}")));
                }
                json_cfg.targets.remove(index);
                crate::config::save(&json_cfg, cfg)?;
                Ok(crate::Output::ConfigSet)
            }
            Commands::Deliver { psyop } => {
                let db = crate::db::Db::open(cfg)?;
                let summary = drain_queue(&db, psyop.as_deref(), cfg).await?;
                Ok(crate::Output::Api(serde_json::to_string(&summary)?))
            }
        }
    }
}

#[derive(serde::Serialize)]
pub struct DeliverySummary {
    pub pending:   usize,
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
    let pending = rows.len();
    let mut delivered = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let dest: Destination = match serde_json::from_str(&row.target_json) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("malformed target_json: {e}");
                crate::emit::emit(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() });
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };
        let post_ids: Vec<String> = match serde_json::from_str(&row.post_ids_json) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("malformed post_ids_json: {e}");
                crate::emit::emit(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() });
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
                crate::emit::emit(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() });
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
                crate::emit::emit(crate::events::Event::DeliveryFailed { delivery_id: row.id, reason: msg.clone() });
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
            }
        }
    }

    Ok(DeliverySummary { pending, delivered, failed })
}
