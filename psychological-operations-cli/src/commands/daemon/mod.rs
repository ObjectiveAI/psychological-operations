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
//!   a shared live store (resolved at reload from their JSONB definitions): a
//!   reload swaps the store (running listeners pick up new hooks immediately)
//!   and starts/stops gateway listeners as agents gain or lose eligibility
//!   (`gateway_raw` is idempotent per agent). Each hook is either `python` (run
//!   for every event) or a declarative trigger (`mention` / `reply` / `dm`)
//!   evaluated against `MESSAGE_CREATE`; a declarative match enqueues the
//!   message for the agent and notifies it. All declarative hooks for one
//!   gateway event are evaluated, then a SINGLE `agents queue deliver` wakes the
//!   agent — one delivery across every hook that matched, not one per match.
//!
//! * **Psyop scheduler** (the main loop): repeatedly does a bare `psyops run`
//!   (every due psyop; manual ones skipped), ignores the result, then sleeps
//!   `max(shortest psyop interval, rand[30..=600]s)`. `run_all` self-delivers
//!   once at the end of each run, so the scheduler issues no deliver of its own.
//!   It is independent of reloads — psyop changes do not notify, and a hook/auth
//!   reload never disturbs scheduling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use objectiveai_sdk::cli::command::agents::queue::deliver;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request};
use psychological_operations_db::{unix_now, Db, DiscordQueueEntry};
use psychological_operations_sdk::cli::hooks::Hook;
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::{self, serenity};
use rand::Rng;
use serenity::all::{Context as SerenityContext, Event, Message, RawEventHandler, UserId};
use tokio::sync::RwLock;

use crate::commands::agents::notify::notify_agent;
use crate::error::Error;

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

/// A hook resolved for the live store. Declarative `user_id`s are already
/// filled from the agent's default and parsed to a [`UserId`]; Python carries
/// its source verbatim.
enum LiveHook {
    Python(String),
    Mention { user_id: UserId, message: String },
    Reply { user_id: UserId, message: String },
    Dm { user_id: UserId, message: String },
}

/// agent_tag -> the agent's resolved hooks. Shared between the gateway handlers
/// (read on every event) and reload (replaces it wholesale).
type HookStore = Arc<RwLock<HashMap<String, Arc<Vec<LiveHook>>>>>;

/// Per-agent raw-event handler: evaluates every hook for every gateway event,
/// reading the current hooks from the shared live store. Python hooks run on
/// every event; declarative hooks act only on `MESSAGE_CREATE`.
struct HookHandler {
    executor: Arc<PluginExecutor>,
    db: Db,
    client: discord::Client,
    store: HookStore,
    agent_tag: String,
}

#[serenity::async_trait]
impl RawEventHandler for HookHandler {
    async fn raw_event(&self, _ctx: SerenityContext, ev: Event) {
        // Latest hooks for this agent (cloned out so we don't hold the lock).
        let Some(hooks) = self.store.read().await.get(&self.agent_tag).cloned() else {
            return;
        };
        if hooks.is_empty() {
            return;
        }

        // Python hooks take the serenity event as their `input`. Serialize once,
        // lazily — only if at least one Python hook is present.
        let py_input = if hooks.iter().any(|h| matches!(h, LiveHook::Python(_))) {
            match serde_json::to_value(&ev) {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!(
                        "discord daemon [{}]: failed to serialize event: {e}",
                        self.agent_tag
                    );
                    None
                }
            }
        } else {
            None
        };

        // Declarative hooks only act on message creation.
        let msg: Option<&Message> = match &ev {
            Event::MessageCreate(mce) => Some(&mce.message),
            _ => None,
        };

        // Track whether any declarative hook enqueued+notified this event, so
        // we issue exactly ONE deliver at the end (across all matched hooks)
        // rather than one per match.
        let mut any_matched = false;

        for hook in hooks.iter() {
            match hook {
                LiveHook::Python(code) => {
                    // Fire-and-forget: spawn so the gateway loop isn't blocked.
                    let Some(input) = py_input.clone() else { continue };
                    let executor = self.executor.clone();
                    let req = Request {
                        path_type: PyPath::Python,
                        code: code.clone(),
                        input: Some(input),
                        no_objectiveai: None,
                        base: Default::default(),
                    };
                    tokio::spawn(async move {
                        let _ = python::execute(&*executor, req, None).await;
                    });
                }
                LiveHook::Mention { user_id, message } => {
                    if let Some(msg) = msg {
                        // Awaited inline (not spawned): the single end-of-event
                        // deliver needs to know whether this hook matched.
                        any_matched |= self.dispatch_mention(msg, *user_id, message).await;
                    }
                }
                LiveHook::Reply { user_id, message } => {
                    if let Some(msg) = msg {
                        // Self-filter: never fire on the watched user's own posts.
                        if msg.author.id != *user_id
                            && msg.referenced_message.as_ref().map(|r| r.author.id)
                                == Some(*user_id)
                        {
                            any_matched |= self.enqueue(msg, message).await;
                        }
                    }
                }
                LiveHook::Dm { user_id, message } => {
                    if let Some(msg) = msg {
                        // A DM has no guild; never fire on the watched user's own.
                        if msg.author.id != *user_id && msg.guild_id.is_none() {
                            any_matched |= self.enqueue(msg, message).await;
                        }
                    }
                }
            }
        }

        // One deliver across ALL hooks that matched this event — wake the agent
        // now rather than waiting for the next scheduler cycle. Scoped to our
        // notify key; fire-and-forget (`execute()` writes the command before it
        // returns, so we drop the response stream — see the scheduler note).
        if any_matched {
            let deliver = deliver::Request {
                path_type: deliver::Path::AgentsQueueDeliver,
                keys: Some(vec![crate::commands::agents::notify::NOTIFY_KEY.to_string()]),
                dangerous_advanced: None,
                base: Default::default(),
            };
            let _ = deliver::execute(&*self.executor, deliver, None).await;
        }
    }
}

impl HookHandler {
    /// Mention match + enqueue; returns whether it enqueued+notified. The
    /// `@everyone` / direct-mention checks are synchronous; a role mention needs
    /// one `get_member` fetch, awaited inline so the caller learns the outcome
    /// (the per-event deliver is issued once, after every hook is evaluated).
    async fn dispatch_mention(&self, msg: &Message, user_id: UserId, message: &str) -> bool {
        // Self-filter: never fire on the watched user's own posts.
        if msg.author.id == user_id {
            return false;
        }
        if msg.mention_everyone || msg.mentions.iter().any(|u| u.id == user_id) {
            return self.enqueue(msg, message).await;
        }
        // Role mention: the only case that needs a fetch — does `user_id` hold
        // one of the mentioned roles?
        if msg.mention_roles.is_empty() {
            return false;
        }
        let Some(guild) = msg.guild_id else {
            return false;
        };
        let member = match self.client.get_member(&self.agent_tag, guild, user_id).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("discord daemon [{}]: mention get_member: {e}", self.agent_tag);
                return false;
            }
        };
        if member.roles.iter().any(|r| msg.mention_roles.contains(r)) {
            return self.enqueue(msg, message).await;
        }
        false
    }

    /// Enqueue + notify for a matched message; returns whether it succeeded.
    /// Awaited inline by `raw_event` so the single end-of-event deliver knows
    /// whether anything was parked.
    async fn enqueue(&self, msg: &Message, message: &str) -> bool {
        enqueue_and_notify(
            &self.db,
            &self.executor,
            &self.agent_tag,
            msg.channel_id.to_string(),
            msg.id.to_string(),
            message.to_string(),
        )
        .await
    }
}

/// Enqueue the triggering message for `tag` (with `message` as the note) and
/// notify the agent — the same enqueue+notify `agents enqueue discord` does
/// (minus the wake, which `raw_event` batches into one deliver per event). The
/// queue upsert is keyed `(agent_tag, channel_id, message_id)`, so re-triggering
/// the same message is idempotent. Returns `true` once both the enqueue and the
/// notify succeed; errors are logged, not propagated.
async fn enqueue_and_notify(
    db: &Db,
    executor: &Arc<PluginExecutor>,
    tag: &str,
    channel_id: String,
    message_id: String,
    message: String,
) -> bool {
    let entry = DiscordQueueEntry {
        agent_tag: tag.to_string(),
        channel_id,
        message_id,
        psyop: None,
        score: None,
        deliverer_agent_instance_hierarchy: None,
        message: Some(message),
        run_id: None,
        queued_at: unix_now(),
    };
    if let Err(e) = db.discord_queue_enqueue(&entry).await {
        eprintln!("discord daemon [{tag}]: enqueue: {e}");
        return false;
    }
    if let Err(e) = notify_agent(db, executor, tag).await {
        eprintln!("discord daemon [{tag}]: notify: {e}");
        return false;
    }
    true
}

/// Re-query the DB and apply: swap the hook store, then ensure a gateway
/// listener exists for every eligible agent (`gateway_raw` is idempotent, so
/// existing listeners are no-ops and new agents get a fresh one).
async fn do_reload(
    db: &Db,
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

    let mut map: HashMap<String, Arc<Vec<LiveHook>>> = HashMap::with_capacity(agents.len());
    for tag in &agents {
        // The bot's own user id — the default `user_id` for declarative hooks
        // that omit it — is the stored application/client id (no API call).
        let default_user_id = db
            .discord_auth_get(tag)
            .await
            .map_err(|e| Error::Other(format!("auth ({tag}): {e}")))?
            .and_then(|a| a.client_id);

        let live: Vec<LiveHook> = db
            .discord_hook_list(tag)
            .await
            .map_err(|e| Error::Other(format!("list hooks ({tag}): {e}")))?
            .into_iter()
            .filter_map(|h| to_live_hook(h.definition, default_user_id.as_deref(), tag, &h.name))
            .collect();
        map.insert(tag.clone(), Arc::new(live));
    }
    *store.write().await = map;

    for tag in to_drop {
        client.stop_gateway(&tag).await;
        eprintln!("discord daemon: dropped listener for {tag}");
    }

    for tag in agents {
        let handler = HookHandler {
            executor: executor.clone(),
            db: db.clone(),
            client: client.clone(),
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

/// Deserialize a stored hook definition and resolve it for the live store.
/// Returns `None` (after an `eprintln!`) on a malformed definition, an
/// unresolvable default `user_id`, or an unparseable `user_id`.
fn to_live_hook(
    definition: serde_json::Value,
    default_user_id: Option<&str>,
    tag: &str,
    name: &str,
) -> Option<LiveHook> {
    let hook: Hook = match serde_json::from_value(definition) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("discord daemon [{tag}]: hook '{name}' malformed: {e}");
            return None;
        }
    };
    // Resolve a declarative hook's `user_id`: explicit, else the bot's own id.
    let resolve = |user_id: Option<String>| -> Option<UserId> {
        let raw = match user_id.or_else(|| default_user_id.map(str::to_string)) {
            Some(s) => s,
            None => {
                eprintln!(
                    "discord daemon [{tag}]: hook '{name}' omits user_id and the bot's \
                     client_id is unknown — skipping"
                );
                return None;
            }
        };
        match raw.parse::<UserId>() {
            Ok(id) => Some(id),
            Err(_) => {
                eprintln!(
                    "discord daemon [{tag}]: hook '{name}' has invalid user_id '{raw}' — skipping"
                );
                None
            }
        }
    };
    match hook {
        Hook::Python { code } => Some(LiveHook::Python(code)),
        Hook::Mention { user_id, message } => Some(LiveHook::Mention {
            user_id: resolve(user_id)?,
            message,
        }),
        Hook::Reply { user_id, message } => Some(LiveHook::Reply {
            user_id: resolve(user_id)?,
            message,
        }),
        Hook::Dm { user_id, message } => Some(LiveHook::Dm {
            user_id: resolve(user_id)?,
            message,
        }),
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

    let store: HookStore = Arc::new(RwLock::new(HashMap::new()));
    let client = discord::Client::new(ctx.db.clone(), ctx.cache_max_size, ctx.cache_ttl);

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
        // The Twitch IRC listeners live entirely inside this task (stateful,
        // single-owner). They reconcile on the SAME `daemon_reload` NOTIFY —
        // the `twitch_auth` / `twitch_channels` triggers fire it — so a Twitch
        // change re-JOINs / (re)connects without disturbing anything else.
        let mut twitch = twitch::TwitchListeners::new(ctx.db.clone());
        tokio::spawn(async move {
            twitch.reload().await;
            eprintln!("twitch daemon: initial reconcile complete");
            loop {
                match listener.next_reload().await {
                    Ok(()) => {
                        if let Err(e) = do_reload(&db, &executor, &store, &client).await {
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
