//! The daemon's Twitch chat listener.
//!
//! Twitch has no chat-history API, so — unlike the X/Discord read paths — the
//! only way an agent can "read" chat is if something is already listening.
//! This module holds one live IRC (TMI) connection per agent that has a Twitch
//! token AND at least one JOINed channel ([`Db::twitch_daemon_agents`]), buffers
//! every `PRIVMSG` into `twitch_messages` (pruned per channel on insert), and
//! the `twitch` MCP's `list_messages` reads that buffer.
//!
//! [`TwitchListeners::reload`] reconciles the live connections with the DB — it
//! runs on startup and on every `daemon_reload` NOTIFY (the `twitch_auth` /
//! `twitch_channels` triggers fire it), mirroring the Discord gateway reloader.
//! Reconciliation is idempotent: it opens/drops connections as agents gain/lose
//! eligibility, reconnects an agent whose token changed (re-login), and
//! JOIN/PARTs channels as the set changes.

use std::collections::{HashMap, HashSet};

use psychological_operations_db::{unix_now, Db, TwitchMessage};
use tokio::task::JoinHandle;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::ServerMessage;
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

type IrcClient = TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>;

/// One agent's live IRC connection: the client handle (for JOIN/PART), the
/// channels it's currently in, the token it authenticated with (to detect a
/// re-login), and the message-drain task.
struct Conn {
    client: IrcClient,
    joined: HashSet<String>,
    token: String,
    drain: JoinHandle<()>,
}

/// Manages the per-agent IRC connections. Owned by (and only touched from) the
/// daemon's reloader task, so it needs no internal locking.
pub struct TwitchListeners {
    db: Db,
    conns: HashMap<String, Conn>,
}

impl TwitchListeners {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            conns: HashMap::new(),
        }
    }

    /// Reconcile live connections + channel joins with the current DB state.
    pub async fn reload(&mut self) {
        // Eligible agents (token + ≥1 channel) and the full (agent, channel) map.
        let agents: HashSet<String> = match self.db.twitch_daemon_agents().await {
            Ok(a) => a.into_iter().collect(),
            Err(e) => {
                eprintln!("twitch daemon: agents query: {e}");
                return;
            }
        };
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

        // Reconnect if the token changed since we last connected (re-login).
        if let Some(conn) = self.conns.get(tag) {
            if conn.token != token {
                if let Some(old) = self.conns.remove(tag) {
                    old.drain.abort();
                }
            }
        }

        if !self.conns.contains_key(tag) {
            let creds = StaticLoginCredentials::new(login.to_lowercase(), Some(token.clone()));
            let config = ClientConfig::new_simple(creds);
            let (mut incoming, client) = IrcClient::new(config);
            let db = self.db.clone();
            let agent_tag = tag.to_string();
            let drain = tokio::spawn(async move {
                while let Some(message) = incoming.recv().await {
                    if let ServerMessage::Privmsg(m) = message {
                        let msg = TwitchMessage {
                            agent_tag: agent_tag.clone(),
                            channel_login: m.channel_login,
                            message_id: m.message_id,
                            user_id: m.sender.id,
                            user_login: m.sender.login,
                            content: m.message_text,
                            sent_at: unix_now(),
                        };
                        if let Err(e) = db.twitch_messages_insert(&msg).await {
                            eprintln!("twitch daemon [{agent_tag}]: insert: {e}");
                        }
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
