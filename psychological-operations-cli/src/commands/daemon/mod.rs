//! `daemon` — the resident Discord gateway daemon + psyop scheduler.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol — so Python runs through the normal
//! `PluginExecutor` in `ctx.executor`). It takes a process-singleton lock then
//! runs two independent jobs forever:
//!
//! * **Hook/auth reloader** (a spawned task): subscribes to the postgres
//!   `daemon_reload` channel and re-queries hook/auth state on every NOTIFY.
//!   The triggers fire on `discord_hooks` / `discord_auth` changes from any
//!   process — there is no reload socket or writer-side kick. Hooks are held in
//!   a shared live store: a reload swaps the store (running listeners pick up
//!   new hooks immediately) and starts/stops gateway listeners as agents gain
//!   or lose eligibility (`gateway_raw` is idempotent per agent).
//!
//! * **Psyop scheduler** (the main loop): repeatedly does a bare `psyops run`
//!   (every due psyop; manual ones skipped), ignores the result, then sleeps
//!   `max(shortest psyop interval, rand[30..=600]s)`. It is independent of
//!   reloads — psyop changes do not notify, and a hook/auth reload never
//!   disturbs scheduling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use objectiveai_sdk::cli::command::agents::queue::deliver;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request};
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::{self, serenity};
use rand::Rng;
use serenity::all::{Context as SerenityContext, Event, RawEventHandler};
use tokio::sync::RwLock;

use crate::error::Error;

/// The scheduler sleeps for `max(shortest psyop interval, rand[MIN..=MAX])`
/// between `psyops run` cycles. The random floor keeps the daemon from waking
/// faster than `MIN_SLEEP_SECS` (no hot-loop on a perpetually-due psyop) and
/// jitters the cadence; the upper bound caps how long a short-interval psyop
/// can drift.
const MIN_SLEEP_SECS: u64 = 30;
const MAX_SLEEP_SECS: u64 = 600;

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

    // Subscribe BEFORE the initial load, so a hook/auth change racing between
    // LISTEN and the load is never lost — at worst it yields one redundant,
    // idempotent reload.
    let listener = ctx
        .db
        .reload_listener()
        .await
        .map_err(|e| Error::Other(format!("subscribe to daemon_reload: {e}")))?;

    // Initial hook/auth load.
    do_reload(&ctx.db, &ctx.executor, &store, &client).await?;
    eprintln!("discord daemon: initial load complete; subscribed to daemon_reload");

    // The hook/auth reloader runs in its own task, independent of the psyop
    // scheduler below: a hook/auth change re-queries hook state but never
    // disturbs psyop scheduling. It owns clones of everything `do_reload`
    // needs (all cheap: Db/Client clone, Arc bumps) plus the listener, so it's
    // `'static`. The listener sits in a plain loop (no `select!`) because
    // sqlx's `try_recv` is not cancel-safe.
    {
        let db = ctx.db.clone();
        let executor = ctx.executor.clone();
        let store = store.clone();
        let client = client.clone();
        let mut listener = listener;
        tokio::spawn(async move {
            loop {
                match listener.next_reload().await {
                    Ok(()) => {
                        if let Err(e) = do_reload(&db, &executor, &store, &client).await {
                            eprintln!("discord daemon: reload failed: {e}");
                        } else {
                            eprintln!("discord daemon: reloaded");
                        }
                    }
                    Err(e) => {
                        eprintln!("discord daemon: listener error: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    // The psyop scheduler (keeps the process alive). Each cycle: do a bare
    // `psyops run` (no names → runs every due psyop, manual ones skipped),
    // ignore the result, then sleep until the next cycle. The sleep is
    // `max(shortest psyop interval, rand[MIN..=MAX])` — never shorter than the
    // most-frequent psyop needs, never less than the random floor. With no
    // interval psyops, it's just the random duration. Reloads do not affect
    // this loop.
    loop {
        // Bare psyops run; the result is intentionally ignored.
        let _ = crate::psyops::run::run_all(Vec::new(), None, ctx).await;

        // Wake queued agents to deliver whatever the run just enqueued.
        // Fire-and-forget through the plugin executor, exactly like the hook
        // handler's `python::execute`: `execute()` writes the command line
        // before it returns, so the host runs the delivery independently — we
        // drop the response stream and ignore the result. We deliberately do
        // NOT drain the stream to its end: the host only writes a nested
        // command's completion terminator after our stdout EOFs (process
        // exit), so awaiting stream end would block forever; dropping it is
        // safe and the listener reaps the pending entry on the next response.
        let deliver = deliver::Request {
            path_type: deliver::Path::AgentsQueueDeliver,
            dangerous_advanced: None,
            base: Default::default(),
        };
        let _ = deliver::execute(&*ctx.executor, deliver, None).await;

        let min_interval = match ctx.db.psyops_min_interval().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("discord daemon: min-interval query: {e}");
                None
            }
        };
        let rand_secs: u64 = rand::thread_rng().gen_range(MIN_SLEEP_SECS..=MAX_SLEEP_SECS);
        let sleep_secs = match min_interval {
            Some(iv) => (iv as u64).max(rand_secs),
            None => rand_secs,
        };
        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
    }
}
