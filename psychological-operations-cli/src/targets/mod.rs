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
    pub async fn handle(self) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::Get { index } => {
                let cfg = crate::config::load();
                match index {
                    Some(i) => {
                        let entry = cfg.targets.get(i)
                            .ok_or_else(|| crate::error::Error::Other(format!("no target at index {i}")))?;
                        Ok(crate::Output::ConfigGet(serde_json::to_string(entry)?))
                    }
                    None => Ok(crate::Output::ConfigGet(serde_json::to_string(&cfg.targets)?)),
                }
            }
            Commands::Add { json } => {
                let parsed: Destination = serde_json::from_str(&json)?;
                let mut cfg = crate::config::load();
                cfg.targets.push(parsed);
                crate::config::save(&cfg)?;
                Ok(crate::Output::ConfigSet)
            }
            Commands::Del { index } => {
                let mut cfg = crate::config::load();
                if index >= cfg.targets.len() {
                    return Err(crate::error::Error::Other(format!("no target at index {index}")));
                }
                cfg.targets.remove(index);
                crate::config::save(&cfg)?;
                Ok(crate::Output::ConfigSet)
            }
            Commands::Deliver { psyop } => deliver(psyop.as_deref()).await,
        }
    }
}

#[derive(serde::Serialize)]
struct DeliverySummary {
    pending:   usize,
    delivered: usize,
    failed:    usize,
}

async fn deliver(psyop_filter: Option<&str>) -> Result<crate::Output, crate::error::Error> {
    use crate::db::{Db, MediaUrl, Post};
    use crate::psyops::psyop;
    use crate::score::ScoredPost;
    use destinations::{send_one, Subject};

    let db = Db::open()?;
    let rows = db.list_pending_deliveries(psyop_filter)?;
    let pending = rows.len();
    let mut delivered = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let dest: Destination = match serde_json::from_str(&row.target_json) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("malformed target_json: {e}");
                eprintln!("delivery #{} failed: {msg}", row.id);
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };
        let post_ids: Vec<String> = match serde_json::from_str(&row.post_ids_json) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("malformed post_ids_json: {e}");
                eprintln!("delivery #{} failed: {msg}", row.id);
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };

        // Load the psyop from disk at current HEAD; we don't
        // checkout the row's commit_sha. If the psyop file is gone,
        // surface that and move on.
        let psyop_obj = match psyop::load(&row.psyop) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("psyop load failed: {e}");
                eprintln!("delivery #{} failed: {msg}", row.id);
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
                continue;
            }
        };

        // Synthesize stub ScoredPosts from the queued IDs. Sufficient
        // for the X destination (only reads post.id); body-rendering
        // destinations will see empty text since `contents` is
        // dropped after scoring.
        let stubs: Vec<ScoredPost> = post_ids.iter().map(|id| ScoredPost {
            post: Post {
                id: id.clone(),
                handle: String::new(),
                text: String::new(),
                images: Vec::<MediaUrl>::new(),
                videos: Vec::<MediaUrl>::new(),
                created: String::new(),
                likes: 0, retweets: 0, replies: 0, impressions: 0,
            },
            score: 0.0,
        }).collect();
        let stub_refs: Vec<&ScoredPost> = stubs.iter().collect();
        let subject = Subject::Psyop {
            name:   &row.psyop,
            psyop:  &psyop_obj,
            output: &stub_refs,
        };

        match send_one(&dest, &subject).await {
            Ok(()) => {
                db.delete_delivery(row.id)?;
                delivered += 1;
            }
            Err(e) => {
                let msg = e.to_string();
                eprintln!("delivery #{} failed: {msg}", row.id);
                db.bump_delivery_attempt(row.id, &msg)?;
                failed += 1;
            }
        }
    }

    let summary = DeliverySummary { pending, delivered, failed };
    Ok(crate::Output::Api(serde_json::to_string(&summary)?))
}
