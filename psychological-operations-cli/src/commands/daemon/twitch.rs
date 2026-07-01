//! The daemon's Twitch chat listener — reactive, like the Discord gateway.
//!
//! Twitch has no chat-history API, so the daemon holds one live IRC (TMI)
//! connection per agent that has a Twitch token AND at least one JOINed channel
//! ([`Db::twitch_daemon_agents`]). For every `PRIVMSG` on a joined channel it:
//!
//! 1. **Buffers** the message into `twitch_messages` (pruned per channel), which
//!    the `twitch` MCP's `list_messages` reads.
//! 2. **Runs the agent's hooks** — `python` (spawned per message, the message
//!    JSON as input) and `mention` (fires when the chat text `@`-mentions the
//!    watched login, defaulting to the bot's own). On a declarative match
//!    it enqueues the message into `twitch_queue`, notifies the agent, and fires
//!    ONE `agents queue deliver` to wake it — the same enqueue→notify→deliver
//!    path the Discord daemon uses.
//!
//! [`TwitchListeners::reload`] reconciles both the live connections and the hook
//! store with the DB — on startup and on every `daemon_reload` NOTIFY (the
//! `twitch_auth` / `twitch_channels` / `twitch_hooks` triggers fire it),
//! mirroring the Discord gateway reloader.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use objectiveai_sdk::cli::command::agents::queue::deliver;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request as PyRequest};
use psychological_operations_db::{unix_now, Db, TwitchMessage, TwitchQueueEntry};
use psychological_operations_sdk::cli::hooks::TwitchHook;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{PrivmsgMessage, ServerMessage};
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

use crate::commands::agents::notify::{notify_agent, NOTIFY_KEY};

type IrcClient = TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>;

/// The live hooks for one agent, resolved from their stored definitions. Read
/// fresh from the shared store on every message so a reload takes effect at once.
type HookStore = Arc<RwLock<HashMap<String, Arc<Vec<LiveHook>>>>>;

/// A resolved Twitch hook (the mention watch string `@<login>` already built).
enum LiveHook {
    /// Operator Python, run for every message with the message JSON as input.
    Python(String),
    /// Fires when the (lowercased) chat text `@`-mentions the watched login,
    /// i.e. contains `watch` (an `@<login>` token); enqueues the message with
    /// `message` as the note.
    Mention { watch: String, message: String },
}

/// One agent's live IRC connection: the client handle (for JOIN/PART), the
/// channels it's currently in, the token it authenticated with (to detect a
/// re-login), the bot's own login (self-filter + mention default), and the
/// message-drain task.
struct Conn {
    client: IrcClient,
    joined: HashSet<String>,
    token: String,
    drain: JoinHandle<()>,
}

/// Manages the per-agent IRC connections + hook store. Owned by (and only
/// touched from) the daemon's reloader task, so it needs no external locking.
pub struct TwitchListeners {
    db: Db,
    executor: Arc<PluginExecutor>,
    hooks: HookStore,
    conns: HashMap<String, Conn>,
}

impl TwitchListeners {
    pub fn new(db: Db, executor: Arc<PluginExecutor>) -> Self {
        Self {
            db,
            executor,
            hooks: Arc::new(RwLock::new(HashMap::new())),
            conns: HashMap::new(),
        }
    }

    /// Reconcile live connections, channel joins, and the hook store with the
    /// current DB state.
    pub async fn reload(&mut self) {
        // Eligible agents (token + ≥1 channel) and the full (agent, channel) map.
        let agents: HashSet<String> = match self.db.twitch_daemon_agents().await {
            Ok(a) => a.into_iter().collect(),
            Err(e) => {
                eprintln!("twitch daemon: agents query: {e}");
                return;
            }
        };

        // Rebuild the hook store for the eligible agents (definitions + the
        // per-agent mention default resolved from the bot's login).
        self.reload_hooks(&agents).await;

        let all = match self.db.twitch_channels_all().await {
            Ok(a) => a,
            Err(e) => {
                eprintln!("twitch daemon: channels query: {e}");
                return;
            }
        };
        let mut desired: HashMap<String, HashSet<String>> = HashMap::new();
        for (agent, channel) in all {
            if agents.contains(&agent) {
                desired.entry(agent).or_default().insert(channel);
            }
        }

        // Tear down connections for agents no longer eligible.
        let drop_tags: Vec<String> = self
            .conns
            .keys()
            .filter(|t| !desired.contains_key(*t))
            .cloned()
            .collect();
        for tag in drop_tags {
            if let Some(conn) = self.conns.remove(&tag) {
                conn.drain.abort();
                eprintln!("twitch daemon: dropped listener for {tag}");
            }
        }

        // Ensure each eligible agent is connected + JOINed to exactly its set.
        for (tag, channels) in desired {
            self.ensure_agent(&tag, &channels).await;
        }
    }

    /// Re-query every eligible agent's hooks and swap the shared store (running
    /// drain tasks pick the new hooks up on their next message).
    async fn reload_hooks(&self, agents: &HashSet<String>) {
        let mut map: HashMap<String, Arc<Vec<LiveHook>>> = HashMap::with_capacity(agents.len());
        for tag in agents {
            // The bot's own login backs the default mention keyword.
            let default_login = match self.db.twitch_auth_get(tag).await {
                Ok(Some(a)) => a.login,
                Ok(None) => None,
                Err(e) => {
                    eprintln!("twitch daemon [{tag}]: auth query: {e}");
                    None
                }
            };
            let entries = match self.db.twitch_hook_list(tag).await {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("twitch daemon [{tag}]: hook list: {e}");
                    continue;
                }
            };
            let live: Vec<LiveHook> = entries
                .into_iter()
                .filter_map(|h| to_live_hook(h.definition, default_login.as_deref(), tag, &h.name))
                .collect();
            map.insert(tag.clone(), Arc::new(live));
        }
        *self.hooks.write().await = map;
    }

    /// Ensure `tag` has a live connection authenticated with its current token
    /// and JOINed to exactly `channels`.
    async fn ensure_agent(&mut self, tag: &str, channels: &HashSet<String>) {
        let auth = match self.db.twitch_auth_get(tag).await {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => {
                eprintln!("twitch daemon [{tag}]: auth query: {e}");
                return;
            }
        };
        let (Some(login), Some(token)) = (auth.login, auth.access_token) else {
            return;
        };
        let login = login.to_lowercase();

        // Reconnect if the token changed since we last connected (re-login).
        if let Some(conn) = self.conns.get(tag) {
            if conn.token != token {
                if let Some(old) = self.conns.remove(tag) {
                    old.drain.abort();
                }
            }
        }

        if !self.conns.contains_key(tag) {
            let creds = StaticLoginCredentials::new(login.clone(), Some(token.clone()));
            let config = ClientConfig::new_simple(creds);
            let (mut incoming, client) = IrcClient::new(config);
            let db = self.db.clone();
            let executor = self.executor.clone();
            let hooks = self.hooks.clone();
            let agent_tag = tag.to_string();
            let bot_login = login.clone();
            let drain = tokio::spawn(async move {
                while let Some(message) = incoming.recv().await {
                    if let ServerMessage::Privmsg(m) = message {
                        process_message(&db, &executor, &hooks, &agent_tag, &bot_login, m).await;
                    }
                }
            });
            self.conns.insert(
                tag.to_string(),
                Conn {
                    client,
                    joined: HashSet::new(),
                    token,
                    drain,
                },
            );
            eprintln!("twitch daemon: listening as {tag} ({login})");
        }

        let conn = self
            .conns
            .get_mut(tag)
            .expect("just inserted or already present");

        // JOIN newly-added channels.
        for channel in channels {
            if !conn.joined.contains(channel) {
                match conn.client.join(channel.clone()) {
                    Ok(()) => {
                        conn.joined.insert(channel.clone());
                    }
                    Err(e) => eprintln!("twitch daemon [{tag}]: join {channel}: {e}"),
                }
            }
        }
        // PART channels no longer in the set.
        let to_part: Vec<String> = conn
            .joined
            .iter()
            .filter(|c| !channels.contains(*c))
            .cloned()
            .collect();
        for channel in to_part {
            conn.client.part(channel.clone());
            conn.joined.remove(&channel);
        }
    }
}

/// Buffer one message, then run the agent's hooks against it.
async fn process_message(
    db: &Db,
    executor: &Arc<PluginExecutor>,
    hooks: &HookStore,
    agent_tag: &str,
    bot_login: &str,
    m: PrivmsgMessage,
) {
    let sent_at = unix_now();

    // 1. Always buffer (the MCP read path).
    let buffered = TwitchMessage {
        agent_tag: agent_tag.to_string(),
        channel_login: m.channel_login.clone(),
        message_id: m.message_id.clone(),
        user_id: m.sender.id.clone(),
        user_login: m.sender.login.clone(),
        content: m.message_text.clone(),
        sent_at,
    };
    if let Err(e) = db.twitch_messages_insert(&buffered).await {
        eprintln!("twitch daemon [{agent_tag}]: insert: {e}");
    }

    // 2. Evaluate hooks (read the latest set from the shared store).
    let Some(agent_hooks) = hooks.read().await.get(agent_tag).cloned() else {
        return;
    };
    if agent_hooks.is_empty() {
        return;
    }

    // Never let a declarative hook fire on the bot's own messages.
    let is_self = m.sender.login.eq_ignore_ascii_case(bot_login);
    let text_lc = m.message_text.to_lowercase();
    let mut matched_note: Option<String> = None;

    for hook in agent_hooks.iter() {
        match hook {
            LiveHook::Python(code) => {
                // Fire-and-forget: spawn so the drain loop isn't blocked.
                let input = serde_json::json!({
                    "channel_login": m.channel_login,
                    "message_id": m.message_id,
                    "user_id": m.sender.id,
                    "user_login": m.sender.login,
                    "content": m.message_text,
                    "sent_at": sent_at,
                });
                let executor = executor.clone();
                let req = PyRequest {
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
            LiveHook::Mention { watch, message } => {
                if !is_self && text_lc.contains(watch) && matched_note.is_none() {
                    matched_note = Some(message.clone());
                }
            }
        }
    }

    // 3. One deliver across all matched declarative hooks — enqueue the message,
    //    notify the agent, then wake it (mirrors the Discord daemon).
    let Some(note) = matched_note else {
        return;
    };
    let entry = TwitchQueueEntry {
        agent_tag: agent_tag.to_string(),
        channel_login: m.channel_login,
        message_id: m.message_id,
        psyop: None,
        score: None,
        deliverer_agent_instance_hierarchy: None,
        message: Some(note),
        run_id: None,
        queued_at: sent_at,
    };
    if let Err(e) = db.twitch_queue_enqueue(&entry).await {
        eprintln!("twitch daemon [{agent_tag}]: enqueue: {e}");
        return;
    }
    if let Err(e) = notify_agent(db, executor, agent_tag).await {
        eprintln!("twitch daemon [{agent_tag}]: notify: {e}");
        return;
    }
    let deliver = deliver::Request {
        path_type: deliver::Path::AgentsQueueDeliver,
        keys: Some(vec![NOTIFY_KEY.to_string()]),
        dangerous_advanced: None,
        base: Default::default(),
    };
    let _ = deliver::execute(&**executor, deliver, None).await;
}

/// Resolve a stored hook definition into a [`LiveHook`]. Returns `None` (after
/// an `eprintln!`) on a malformed definition. A `mention` hook's `keyword`
/// defaults to the bot's own `@<login>` (lowercased for case-insensitive match).
fn to_live_hook(
    definition: serde_json::Value,
    default_login: Option<&str>,
    tag: &str,
    name: &str,
) -> Option<LiveHook> {
    let hook: TwitchHook = match serde_json::from_value(definition) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("twitch daemon [{tag}]: hook '{name}' malformed: {e}");
            return None;
        }
    };
    match hook {
        TwitchHook::Python { code } => Some(LiveHook::Python(code)),
        TwitchHook::Mention {
            user_login,
            message,
        } => {
            // The login whose @-mentions we watch — an explicit one, else the
            // bot's own. Matched as the lowercased `@<login>` token.
            let login = match user_login.or_else(|| default_login.map(str::to_string)) {
                Some(l) => l,
                None => {
                    eprintln!(
                        "twitch daemon [{tag}]: mention hook '{name}' omits user_login and the \
                         bot's login is unknown — skipping"
                    );
                    return None;
                }
            };
            Some(LiveHook::Mention {
                watch: format!("@{}", login.to_lowercase()),
                message,
            })
        }
    }
}
