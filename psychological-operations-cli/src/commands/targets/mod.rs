//! `targets` subcommand surface.
//!
//! The `Destination` enum + per-destination `send_one` impls + the
//! `drain_queue` runtime hook all live in `crate::targets` — this
//! file owns the clap surface and the dispatch for the four
//! subcommands.

use clap::Subcommand;
use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::targets::destinations::Destination;

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
    pub async fn handle(self, cfg: &crate::run::Config) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                Commands::Get { index } => {
                    let json_cfg = crate::config::load(cfg);
                    match index {
                        Some(i) => {
                            let entry = json_cfg.targets.get(i).ok_or_else(|| {
                                Error::Other(format!("no target at index {i}"))
                            })?;
                            Ok(Output::ConfigGet(serde_json::to_string(entry)?))
                        }
                        None => Ok(Output::ConfigGet(
                            serde_json::to_string(&json_cfg.targets)?,
                        )),
                    }
                }
                Commands::Add { json } => {
                    let parsed: Destination = serde_json::from_str(&json)?;
                    let mut json_cfg = crate::config::load(cfg);
                    json_cfg.targets.push(parsed);
                    crate::config::save(&json_cfg, cfg)?;
                    Ok(Output::ConfigSet)
                }
                Commands::Del { index } => {
                    let mut json_cfg = crate::config::load(cfg);
                    if index >= json_cfg.targets.len() {
                        return Err(Error::Other(format!("no target at index {index}")));
                    }
                    json_cfg.targets.remove(index);
                    crate::config::save(&json_cfg, cfg)?;
                    Ok(Output::ConfigSet)
                }
                Commands::Deliver { psyop } => {
                    let db = crate::db::Db::open(cfg)?;
                    let summary = crate::targets::drain_queue(&db, psyop.as_deref(), cfg).await?;
                    Ok(Output::Api(serde_json::to_string(&summary)?))
                }
            }
        }.await;
        crate::output::emit_result(result)
    }
}
