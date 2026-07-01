//! The daemon's Discord gateway listener.
//!
//! A shared live hook store (`agent_tag -> resolved hooks`) is read by every
//! per-agent gateway handler on every event and swapped wholesale on reload.
//! Python hooks run for every gateway event; the declarative triggers
//! (`mention` / `reply` / `dm`) are evaluated in Rust against `MESSAGE_CREATE`,
//! and a match enqueues the triggering message for the agent + notifies it, with
//! ONE `agents queue deliver` per event waking it (the same enqueue→notify→
//! deliver path the Twitch listener uses).
//!
//! [`do_reload`] reconciles the hook store + gateway listeners with the DB; the
//! daemon's reloader task (in the parent module) calls it on startup and on
//! every `daemon_reload` NOTIFY. [`new_store`] / [`new_client`] are the small
//! constructors the parent's `begin` uses to wire it up.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use objectiveai_sdk::cli::command::agents::queue::deliver;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request};
use psychological_operations_db::{unix_now, Db, DiscordQueueEntry};
use psychological_operations_sdk::cli::hooks::Hook;
use psychological_operations_sdk::discord::{self, serenity};
use serenity::all::{Context as SerenityContext, Event, Message, RawEventHandler, UserId};
use tokio::sync::RwLock;

use crate::commands::agents::notify::notify_agent;
use crate::error::Error;

/// A hook resolved for the live store. Declarative `user_id`s are already
/// filled from the agent's default and parsed to a [`UserId`]; Python carries
/// its source verbatim. `pub(super)` only because it appears in the
/// [`HookStore`] alias `begin` names; nothing outside this module touches it.
pub(super) enum LiveHook {
    Python(String),
    Mention { user_id: UserId, message: String },
    Reply { user_id: UserId, message: String },
    Dm { user_id: UserId, message: String },
}

/// agent_tag -> the agent's resolved hooks. Shared between the gateway handlers
/// (read on every event) and reload (replaces it wholesale).
pub(super) type HookStore = Arc<RwLock<HashMap<String, Arc<Vec<LiveHook>>>>>;

/// Construct an empty hook store.
pub(super) fn new_store() -> HookStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Construct the Discord SDK client the daemon acts through.
pub(super) fn new_client(db: Db, cache_max_size: u64, cache_ttl: Duration) -> discord::Client {
    discord::Client::new(db, cache_max_size, cache_ttl)
}

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
pub(super) async fn do_reload(
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
