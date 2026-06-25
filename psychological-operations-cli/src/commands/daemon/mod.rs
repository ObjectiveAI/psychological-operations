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
//!   message for the agent and notifies it, like `agents enqueue discord`.
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
use psychological_operations_db::{unix_now, Db, DiscordQueueEntry};
use psychological_operations_sdk::cli::hooks::Hook;
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::{self, serenity};
use rand::Rng;
use serenity::all::{Context as SerenityContext, Event, Message, RawEventHandler, UserId};
use tokio::sync::RwLock;

use crate::commands::agents::notify::notify_agent;
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
                        self.dispatch_mention(msg, *user_id, message);
                    }
                }
                LiveHook::Reply { user_id, message } => {
                    if let Some(msg) = msg {
                        // Self-filter: never fire on the watched user's own posts.
                        if msg.author.id == *user_id {
                            continue;
                        }
                        if msg.referenced_message.as_ref().map(|r| r.author.id) == Some(*user_id) {
                            self.spawn_enqueue(msg, message);
                        }
                    }
                }
                LiveHook::Dm { user_id, message } => {
                    if let Some(msg) = msg {
                        if msg.author.id == *user_id {
                            continue;
                        }
                        // A DM has no guild.
                        if msg.guild_id.is_none() {
                            self.spawn_enqueue(msg, message);
                        }
                    }
                }
            }
        }
    }
}

impl HookHandler {
    /// Mention match + enqueue. The `@everyone` / direct-mention checks are
    /// synchronous; a role mention needs one `get_member` fetch, done in a
    /// spawned task so the gateway loop isn't blocked.
    fn dispatch_mention(&self, msg: &Message, user_id: UserId, message: &str) {
        // Self-filter: never fire on the watched user's own posts.
        if msg.author.id == user_id {
            return;
        }
        if msg.mention_everyone || msg.mentions.iter().any(|u| u.id == user_id) {
            self.spawn_enqueue(msg, message);
            return;
        }
        // Role mention: the only case that needs a fetch — does `user_id` hold
        // one of the mentioned roles?
        if msg.mention_roles.is_empty() {
            return;
        }
        let Some(guild) = msg.guild_id else {
            return;
        };
        let mention_roles = msg.mention_roles.clone();
        let channel_id = msg.channel_id.to_string();
        let message_id = msg.id.to_string();
        let message = message.to_string();
        let client = self.client.clone();
        let db = self.db.clone();
        let executor = self.executor.clone();
        let tag = self.agent_tag.clone();
        tokio::spawn(async move {
            let member = match client.get_member(&tag, guild, user_id).await {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("discord daemon [{tag}]: mention get_member: {e}");
                    return;
                }
            };
            if member.roles.iter().any(|r| mention_roles.contains(r)) {
                enqueue_and_notify(&db, &executor, &tag, channel_id, message_id, message).await;
            }
        });
    }

    /// Spawn the enqueue + notify for a matched message.
    fn spawn_enqueue(&self, msg: &Message, message: &str) {
        let db = self.db.clone();
        let executor = self.executor.clone();
        let tag = self.agent_tag.clone();
        let channel_id = msg.channel_id.to_string();
        let message_id = msg.id.to_string();
        let message = message.to_string();
        tokio::spawn(async move {
            enqueue_and_notify(&db, &executor, &tag, channel_id, message_id, message).await;
        });
    }
}

/// Enqueue the triggering message for `tag` (with `message` as the note) and
/// notify the agent — exactly what `agents enqueue discord` does. The queue
/// upsert is keyed `(agent_tag, channel_id, message_id)`, so re-triggering the
/// same message is idempotent. Errors are logged, not propagated.
async fn enqueue_and_notify(
    db: &Db,
    executor: &Arc<PluginExecutor>,
    tag: &str,
    channel_id: String,
    message_id: String,
    message: String,
) {
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
        return;
    }
    if let Err(e) = notify_agent(db, executor, tag).await {
        eprintln!("discord daemon [{tag}]: notify: {e}");
    }
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
            // Only deliver our own parked psyop/hook notifications (the key
            // `notify_agent` enqueues under), not unrelated pending messages.
            keys: Some(vec![crate::commands::agents::notify::NOTIFY_KEY.to_string()]),
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
