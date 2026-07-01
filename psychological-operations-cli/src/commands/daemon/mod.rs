//! `daemon` — the resident Discord + Twitch daemon + psyop scheduler.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol — so Python runs through the normal
//! `PluginExecutor` in `ctx.executor`). It takes a process-singleton lock then
//! runs two independent jobs forever:
//!
//! * **Hook/auth reloader** (a spawned task): subscribes to the postgres
//!   `daemon_reload` channel and reconciles listener state on every NOTIFY. It
//!   drives BOTH platform listeners — the Discord gateway ([`discord`]) and the
//!   Twitch IRC chat listener ([`twitch`]) — off the one channel: the triggers
//!   fire on `discord_hooks`/`discord_auth` and `twitch_auth`/`twitch_channels`/
//!   `twitch_hooks` changes from any process. Each listener resolves its hooks
//!   from their JSONB definitions, swaps its live store, starts/stops per-agent
//!   connections as agents gain or lose eligibility (idempotent), and on a hook
//!   match enqueues the triggering message + notifies the agent, with ONE
//!   `agents queue deliver` per event waking it.
//!
//! * **Psyop scheduler** (the main loop): repeatedly does a bare `psyops run`
//!   (every due psyop; manual ones skipped), ignores the result, then sleeps
//!   `max(shortest psyop interval, rand[30..=600]s)`. `run_all` self-delivers
//!   once at the end of each run, so the scheduler issues no deliver of its own.
//!   It is independent of reloads — psyop changes do not notify, and a reload
//!   never disturbs scheduling.

use std::time::Duration;

use clap::Subcommand;
use psychological_operations_sdk::cli::Output as CliOutput;
use rand::Rng;

use crate::error::Error;

mod discord;
mod twitch;

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

    let store = discord::new_store();
    let client = discord::new_client(ctx.db.clone(), ctx.cache_max_size, ctx.cache_ttl);

    // Subscribe BEFORE the initial load, so a hook/auth change racing between
    // LISTEN and the load is never lost — at worst it yields one redundant,
    // idempotent reload.
    let listener = ctx
        .db
        .reload_listener()
        .await
        .map_err(|e| Error::Other(format!("subscribe to daemon_reload: {e}")))?;

    // Initial hook/auth load.
    discord::do_reload(&ctx.db, &ctx.executor, &store, &client).await?;
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
        // The Twitch IRC listeners live entirely inside this task (stateful,
        // single-owner). They reconcile on the SAME `daemon_reload` NOTIFY —
        // the `twitch_auth` / `twitch_channels` / `twitch_hooks` triggers fire
        // it — so a Twitch change re-JOINs / (re)connects without disturbing
        // anything else.
        let mut twitch = twitch::TwitchListeners::new(ctx.db.clone(), ctx.executor.clone());
        tokio::spawn(async move {
            twitch.reload().await;
            eprintln!("twitch daemon: initial reconcile complete");
            loop {
                match listener.next_reload().await {
                    Ok(()) => {
                        if let Err(e) = discord::do_reload(&db, &executor, &store, &client).await {
                            eprintln!("discord daemon: reload failed: {e}");
                        } else {
                            eprintln!("discord daemon: reloaded");
                        }
                        twitch.reload().await;
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
        // Bare psyops run; the result is intentionally ignored. `run_all`
        // self-delivers once at the end (waking every agent it notified), so
        // the scheduler no longer issues its own `agents queue deliver`.
        let _ = crate::psyops::run::run_all(Vec::new(), None, ctx).await;

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
