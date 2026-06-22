//! `psyops run` — execute one or more psyops end-to-end, fully in memory.
//!
//! This module is the platform-agnostic orchestrator. It resolves which
//! psyops to run (an explicit `--name` list, or every enabled psyop), drops
//! any that fail validation / trigger gating / locking, partitions the
//! survivors by family, and runs the X and Discord batches concurrently. Each
//! family's batch ([`x::run_batch`] / [`discord::run_batch`]) owns its own
//! pipeline (scrape/ingest → filter → sort → dedup → score → deliver) and
//! emits a `PsyopRunFailed` event per psyop it can't complete.
//!
//! **Trigger gating.** A psyop's `trigger` is either `manual` or `interval`:
//!
//! * `manual` — runs **only** when explicitly named. With no `--name`, manual
//!   psyops are skipped silently.
//! * `interval` — runs on cadence. It's skipped (with `PsyopSkippedInterval`
//!   for named runs) until its humantime interval has elapsed since the last
//!   successful run.
//!
//! NOTHING about the candidate pipeline is persisted — posts, hydration, and
//! scores all live in memory for the lifetime of this call. Only the per-psyop
//! interval stamp (`psyop_runs`), the delivered-once ledgers, and the agent
//! queues are durable.

pub mod discord;
pub mod x;

use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::cli::psyops::PsyOp;

use crate::error::Error;

/// CLI entrypoint for `psyops::Commands::Run`.
pub async fn run_all(names: Vec<String>, seed: Option<i64>, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_all_inner(names, seed, ctx).await)
}

async fn run_all_inner(
    names: Vec<String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let db = &ctx.db;
    let names_given = !names.is_empty();

    // Load (name, PsyOp) pairs. Names given → load each by name. No names →
    // every enabled psyop.
    let loaded: Vec<(String, PsyOp)> = if names_given {
        let mut loaded = Vec::new();
        for name in names {
            match super::psyop::load(&name, ctx).await {
                Ok(psyop) => loaded.push((name, psyop)),
                Err(e) => emit_run_failed(&name, &e.to_string()),
            }
        }
        loaded
    } else {
        let mut loaded = Vec::new();
        for (name, def, _disabled) in db
            .psyop_list()
            .await?
            .into_iter()
            .filter(|(_, _, disabled)| !disabled)
        {
            match serde_json::from_value::<PsyOp>(def) {
                Ok(psyop) => loaded.push((name, psyop)),
                Err(e) => emit_run_failed(&name, &e.to_string()),
            }
        }
        loaded
    };

    // Per-psyop file locks (`<state_dir>/psyops/locks/<psyop>`): a
    // non-blocking guard so two concurrent `psyops run` invocations never
    // process the same psyop. Held for the life of this run and released at
    // the end (drop alone is a no-op for the lockfile API — the OS otherwise
    // frees them only on process exit).
    let locks_dir = ctx.config.state_dir().join("psyops").join("locks");
    let mut claims: Vec<objectiveai_sdk::lockfile::LockClaim> = Vec::new();

    // Resolve the runnable set: validate → trigger gate → lock. Each
    // non-runnable psyop emits its own event and is dropped; only db errors
    // abort the batch. Survivors are split by family for concurrent runs.
    let mut x_runnable: Vec<(String, psychological_operations_sdk::cli::psyops::x::PsyOp)> =
        Vec::new();
    let mut discord_runnable: Vec<(
        String,
        psychological_operations_sdk::cli::psyops::discord::PsyOp,
    )> = Vec::new();

    for (name, psyop) in loaded {
        if let Err(reason) = psyop.validate() {
            crate::output::OutputResult::from(crate::events::Event::PsyopInvalidAtRun {
                psyop: name.clone(),
                reason,
            })
            .emit();
            continue;
        }

        // Trigger gate. `None` ⇒ manual (run only when named); `Some(dur)` ⇒
        // interval (run when the cadence has elapsed).
        match psyop.trigger_interval() {
            Err(reason) => {
                crate::output::OutputResult::from(crate::events::Event::PsyopInvalidAtRun {
                    psyop: name.clone(),
                    reason,
                })
                .emit();
                continue;
            }
            Ok(None) => {
                // Manual: skip entirely unless this psyop was explicitly named.
                if !names_given {
                    continue;
                }
            }
            Ok(Some(interval)) => {
                if let Some(last_run) = db.get_last_run(&name).await? {
                    let elapsed = (chrono::Utc::now().timestamp() - last_run).max(0) as u64;
                    if elapsed < interval.as_secs() {
                        if names_given {
                            crate::output::OutputResult::from(
                                crate::events::Event::PsyopSkippedInterval {
                                    psyop: name.clone(),
                                    interval: humantime::format_duration(interval).to_string(),
                                    remaining_secs: interval.as_secs() - elapsed,
                                },
                            )
                            .emit();
                        }
                        continue;
                    }
                }
            }
        }

        // Take the psyop's non-blocking file lock. If another run already
        // holds it, skip (announce only for explicitly-named runs, mirroring
        // the interval skip). Psyop names are flat; collapse any stray
        // separator so the key is a single filesystem segment.
        let lock_key = name.replace(['/', '\\'], "-");
        match objectiveai_sdk::lockfile::try_acquire(
            &locks_dir,
            &lock_key,
            &format!("pid {} psyops run", std::process::id()),
        )
        .await
        {
            Some(claim) => claims.push(claim),
            None => {
                if names_given {
                    crate::output::OutputResult::from(crate::events::Event::PsyopSkippedLocked {
                        psyop: name.clone(),
                    })
                    .emit();
                }
                continue;
            }
        }

        match psyop {
            PsyOp::X(p) => x_runnable.push((name, p)),
            PsyOp::Discord(p) => discord_runnable.push((name, p)),
        }
    }

    // Run both families concurrently. Each batch runs its psyops in parallel
    // internally and emits `PsyopRunFailed` per psyop it can't complete, so
    // neither call returns an error.
    tokio::join!(
        x::run_batch(x_runnable, seed, ctx),
        discord::run_batch(discord_runnable, seed, ctx),
    );

    // Release the per-psyop locks now the batch is done (drop is a no-op for
    // the lockfile API; without this they'd free only on process exit).
    for claim in claims {
        let _ = claim.release();
    }

    Ok(Output::Ok)
}

/// Emit a non-fatal per-psyop failure event (the batch keeps running). Shared
/// by the orchestrator and both family batches.
pub(super) fn emit_run_failed(psyop: &str, error: &str) {
    crate::output::OutputResult::from(crate::events::Event::PsyopRunFailed {
        psyop: psyop.to_string(),
        error: error.to_string(),
    })
    .emit();
}
