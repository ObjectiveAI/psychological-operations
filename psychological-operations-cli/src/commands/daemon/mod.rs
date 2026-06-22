//! `daemon` — the resident Discord gateway daemon.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol — so Python runs through the normal
//! `PluginExecutor` in `ctx.executor`). It takes a process-singleton lock,
//! does an initial load, then subscribes to the database and reloads on every
//! notification, indefinitely.
//!
//! Reload is driven by the database itself: postgres triggers on `psyops`,
//! `discord_hooks`, and `discord_auth` fire a `daemon_reload` NOTIFY on any
//! change (from any process), which the daemon receives via a [`ReloadListener`]
//! ([`psychological_operations_db::Db::reload_listener`]). There is no longer a
//! reload socket or a writer-side kick — the mutating commands just write to
//! the DB and the trigger does the rest.
//!
//! Hooks are held in a shared, live store rather than snapshotted: a reload
//! re-queries the DB, swaps the store (so running listeners pick up the new
//! hooks immediately), and starts gateway listeners for any newly-eligible
//! agents (`gateway_raw` is idempotent per agent). An agent that loses
//! eligibility has its gateway connection torn down.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request};
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::{self, serenity};
use serenity::all::{Context as SerenityContext, Event, RawEventHandler};
use tokio::sync::{Notify, RwLock};

use crate::error::Error;

/// Minimum gap between scheduler-launched `psyops run` invocations. Bounds how
/// often a perpetually-due psyop (one that keeps failing/skipping and so never
/// advances its last-run stamp) is re-attempted; healthy psyops run on their
/// own interval, far above this.
const MIN_CYCLE: Duration = Duration::from_secs(30);

#[derive(Subcommand)]
pub enum Commands {
    /// Run the resident Discord gateway daemon (never returns).
    Begin,
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Begin => crate::output::emit_result(begin(ctx).await),
        }
    }
}

/// agent_tag -> the agent's hooks' Python sources. Shared between the gateway
/// handlers (read on every event) and reload (replaces it wholesale).
type HookStore = Arc<RwLock<HashMap<String, Arc<Vec<String>>>>>;

/// Per-agent raw-event handler: runs every hook for every gateway event,
/// reading the current hooks from the shared live store.
struct HookHandler {
    executor: Arc<PluginExecutor>,
    store: HookStore,
    agent_tag: String,
}

#[serenity::async_trait]
impl RawEventHandler for HookHandler {
    async fn raw_event(&self, _ctx: SerenityContext, ev: Event) {
        // Latest hooks for this agent (cloned out so we don't hold the lock).
        let Some(codes) = self.store.read().await.get(&self.agent_tag).cloned() else {
            return;
        };
        if codes.is_empty() {
            return;
        }
        // The serenity event is the hook's `input`. Serialize once.
        let input = match serde_json::to_value(&ev) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "discord daemon [{}]: failed to serialize event: {e}",
                    self.agent_tag
                );
                return;
            }
        };
        for code in codes.iter() {
            // Fire-and-forget: spawn so the gateway loop isn't blocked, and
            // ignore the result (we only care that it runs).
            let executor = self.executor.clone();
            let req = Request {
                path_type: PyPath::Python,
                code: code.clone(),
                input: Some(input.clone()),
                base: Default::default(),
            };
            tokio::spawn(async move {
                let _ = python::execute(&*executor, req, None).await;
            });
        }
    }
}

/// Re-query the DB and apply: swap the hook store, then ensure a gateway
/// listener exists for every eligible agent (`gateway_raw` is idempotent, so
/// existing listeners are no-ops and new agents get a fresh one).
async fn do_reload(
    db: &psychological_operations_db::Db,
    executor: &Arc<PluginExecutor>,
    store: &HookStore,
    client: &discord::Client,
) -> Result<(), Error> {
    let agents = db
        .discord_daemon_agents()
        .await
        .map_err(|e| Error::Other(format!("list agents: {e}")))?;

    // Agents we had a listener for last reload (the store's keys) that are no
    // longer eligible — tear their gateway connections down.
    let to_drop: Vec<String> = {
        let cur = store.read().await;
        cur.keys()
            .filter(|k| !agents.contains(k))
            .cloned()
            .collect()
    };

    let mut map: HashMap<String, Arc<Vec<String>>> = HashMap::with_capacity(agents.len());
    for tag in &agents {
        let codes: Vec<String> = db
            .discord_hook_list(tag)
            .await
            .map_err(|e| Error::Other(format!("list hooks ({tag}): {e}")))?
            .into_iter()
            .map(|h| h.python)
            .collect();
        map.insert(tag.clone(), Arc::new(codes));
    }
    *store.write().await = map;

    for tag in to_drop {
        client.stop_gateway(&tag).await;
        eprintln!("discord daemon: dropped listener for {tag}");
    }

    for tag in agents {
        let handler = HookHandler {
            executor: executor.clone(),
            store: store.clone(),
            agent_tag: tag.clone(),
        };
        client
            .gateway_raw(&tag, handler)
            .await
            .map_err(|e| Error::Other(format!("gateway ({tag}): {e}")))?;
    }
    Ok(())
}

async fn begin(ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Process-singleton: a second daemon would open duplicate gateway
    // connections for the same bots (which Discord punishes). Bow out if held.
    let state_dir = ctx.config.state_dir();
    let _claim = objectiveai_sdk::lockfile::try_acquire(
        &state_dir.join("locks"),
        "daemon",
        &format!("pid {} discord daemon", std::process::id()),
    )
    .await
    .ok_or_else(|| Error::Other("the Discord daemon is already running".into()))?;

    let store: HookStore = Arc::new(RwLock::new(HashMap::new()));
    let client = discord::Client::new(ctx.db.clone());

    // Subscribe BEFORE the initial load, so a change racing between LISTEN and
    // the load is never lost — at worst it yields one redundant, idempotent
    // reload below.
    let listener = ctx
        .db
        .reload_listener()
        .await
        .map_err(|e| Error::Other(format!("subscribe to daemon_reload: {e}")))?;

    // Pump the listener in its own task: sqlx's `try_recv` is not cancel-safe,
    // so it must never sit in a `select!` arm (a dropped receive can lose a
    // notification). Instead each notification pokes a cancel-safe `Notify`
    // that the scheduler loop selects on. Coalescing is fine — the daemon
    // re-queries everything on any signal.
    let reload = Arc::new(Notify::new());
    {
        let reload = reload.clone();
        let mut listener = listener;
        tokio::spawn(async move {
            loop {
                match listener.next_reload().await {
                    Ok(()) => reload.notify_one(),
                    Err(e) => {
                        eprintln!("discord daemon: listener error: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    // Initial load.
    do_reload(&ctx.db, &ctx.executor, &store, &client).await?;
    eprintln!("discord daemon: initial load complete; subscribed to daemon_reload");

    // The scheduler + reload loop (keeps the process alive). Each iteration:
    // compute when the next interval psyop is due, then race that timer against
    // a reload poke.
    //   * reload poke  → re-load hooks/auth AND restart the scheduler (the loop
    //     recomputes the next-due time); psyops/hooks/auth changes all land here.
    //   * timer fires  → a bare `psyops run` (no names: runs every due psyop,
    //     manual ones skipped); the result is ignored.
    // With no interval psyops, the timer is `pending()` and only a reload wakes
    // the loop, so the daemon idles.
    let mut last_run_started: Option<Instant> = None;
    loop {
        let next_due = match ctx.db.psyops_next_due().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("discord daemon: next-due query: {e}");
                None
            }
        };
        let now = chrono::Utc::now().timestamp();
        let wait = next_due.map(|due| Duration::from_secs((due - now).max(0) as u64));
        let timer = async {
            match wait {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            _ = reload.notified() => {
                if let Err(e) = do_reload(&ctx.db, &ctx.executor, &store, &client).await {
                    eprintln!("discord daemon: reload failed: {e}");
                } else {
                    eprintln!("discord daemon: reloaded");
                }
            }
            _ = timer => {
                // Spin guard: a psyop that fails (or is skipped for auth/lock)
                // never advances its last-run stamp, so it stays "due" forever.
                // Don't launch runs closer together than MIN_CYCLE. Healthy
                // psyops are unaffected — their inter-run gap is the interval,
                // far above MIN_CYCLE.
                if let Some(prev) = last_run_started {
                    let elapsed = prev.elapsed();
                    if elapsed < MIN_CYCLE {
                        tokio::time::sleep(MIN_CYCLE - elapsed).await;
                    }
                }
                last_run_started = Some(Instant::now());
                // Bare psyops run; the result is intentionally ignored.
                let _ = crate::psyops::run::run_all(Vec::new(), None, ctx).await;
            }
        }
    }
}
