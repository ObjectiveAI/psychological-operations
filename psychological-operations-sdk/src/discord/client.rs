//! Discord API client (serenity-backed).
//!
//! Multi-agent: every call takes an `agent_tag` and authenticates from that
//! agent's `discord_auth` row. Two capabilities, both **lazily built + cached
//! per agent** so repeat calls return the cached instance immediately:
//!
//! - **REST** ([`Client::http`]) — a `serenity::http::Http` for regular API
//!   calls (read channel history, send a message, open a DM, …).
//! - **Gateway** ([`Client::gateway`]) — establishes a live gateway
//!   connection with the given intents + [`EventHandler`], runs its event
//!   loop in a background task, and caches the connection's `ShardManager`.
//!
//! Each cache is a `DashMap<agent_tag, OnceCell<resource>>`: the per-tag
//! [`tokio::sync::OnceCell`] guarantees the resource is built **exactly once**
//! per agent even under concurrent calls (so two callers never open duplicate
//! gateway connections for the same bot). The inner state is `Arc`-shared, so
//! cloning the `Client` shares the same caches.
//!
//! Caching means a token change (re-login / reset) is **not** picked up for an
//! already-cached agent — unlike the per-call [`crate::x::client::Client`].

use std::sync::Arc;

use dashmap::DashMap;
use psychological_operations_db::Db;
use serenity::all::{EventHandler, GatewayIntents, ShardManager};
use tokio::sync::OnceCell;

use super::error::Error;

/// A per-agent cache cell: built once, then shared.
type Cache<T> = DashMap<String, Arc<OnceCell<Arc<T>>>>;

/// Discord client. Cheap to clone — clones share the inner caches.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    /// The single persistence layer — holds each agent's `discord_auth` row.
    db: Db,
    /// Lazily-built REST clients, one per agent tag.
    http: Cache<serenity::http::Http>,
    /// Lazily-established gateway connections' shard managers, one per agent
    /// tag. Each connection's event loop runs in a background task.
    gateway: Cache<ShardManager>,
}

impl Client {
    /// Build a Discord client. **Infallible** and **synchronous** — no I/O
    /// happens here. Tokens + resources are resolved lazily, per agent, on the
    /// first `http` / `gateway` call for that agent.
    pub fn new(db: Db) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                http: DashMap::new(),
                gateway: DashMap::new(),
            }),
        }
    }

    /// The shared persistence handle.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Resolve the agent's bot token from the DB. `discord_auth_get(tag)` then
    /// the row's `bot_token`; [`Error::NotAuthed`] if there's no row or no
    /// token.
    async fn bot_token(&self, agent_tag: &str) -> Result<String, Error> {
        self.inner
            .db
            .discord_auth_get(agent_tag)
            .await?
            .and_then(|a| a.bot_token)
            .ok_or_else(|| Error::NotAuthed(agent_tag.to_string()))
    }

    /// The agent's REST client, authed as its bot. Built + cached on the first
    /// call for that agent; later calls return the cached `Arc` immediately.
    pub async fn http(&self, agent_tag: &str) -> Result<Arc<serenity::http::Http>, Error> {
        // Take (or create) the per-tag cell, then drop the map guard before
        // awaiting on it.
        let cell = self.inner.http.entry(agent_tag.to_string()).or_default().clone();
        cell.get_or_try_init(|| async {
            let token = self.bot_token(agent_tag).await?;
            Ok(Arc::new(serenity::http::Http::new(&token)))
        })
        .await
        .cloned()
    }

    /// The agent's live gateway connection's shard manager. On the first call
    /// for that agent this resolves the token, builds the gateway client with
    /// `intents` + `handler`, spawns its event loop in a background task, and
    /// caches the resulting `ShardManager`. Later calls return the cached
    /// handle immediately — **`intents` and `handler` from the first call
    /// stick; the args on later calls are ignored** (one connection per agent).
    pub async fn gateway<H: EventHandler + 'static>(
        &self,
        agent_tag: &str,
        intents: GatewayIntents,
        handler: H,
    ) -> Result<Arc<ShardManager>, Error> {
        let cell = self.inner.gateway.entry(agent_tag.to_string()).or_default().clone();
        cell.get_or_try_init(|| async {
            let token = self.bot_token(agent_tag).await?;
            let mut client = serenity::Client::builder(&token, intents)
                .event_handler(handler)
                .await?;
            let shard_manager = client.shard_manager.clone();
            // Run the event loop for the life of the process; the handler
            // receives events. A start() error (e.g. disconnect) ends the
            // task but leaves the cached manager in place.
            tokio::spawn(async move {
                let _ = client.start().await;
            });
            Ok(shard_manager)
        })
        .await
        .cloned()
    }
}
