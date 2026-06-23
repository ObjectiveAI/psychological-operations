//! Discord API client (serenity-backed).
//!
//! Multi-agent: every call takes an `agent_tag` and authenticates from that
//! agent's `discord_auth` row. Two capabilities, both **lazily built + cached
//! per agent** so repeat calls return the cached instance immediately:
//!
//! - **REST** ([`Client::http`]) — a `serenity::http::Http` for regular API
//!   calls (read channel history, send a message, open a DM, …).
//! - **Gateway** ([`Client::gateway`]) — establishes a live gateway
//!   connection (with **all** intents) + an [`EventHandler`], runs its event
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
use std::time::Duration;

use dashmap::DashMap;
use psychological_operations_db::Db;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serenity::all::{
    CurrentUser, EventHandler, GatewayIntents, GuildChannel, GuildId, GuildInfo, RawEventHandler,
    ShardManager,
};
use tokio::sync::OnceCell;

use super::cache;
use super::error::Error;

/// A per-agent cache cell: built once, then shared.
type Cache<T> = DashMap<String, Arc<OnceCell<Arc<T>>>>;

/// Discord client. Cheap to clone — clones share the inner caches.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    /// The single persistence layer — holds each agent's `discord_auth` row,
    /// and backs the response cache (`cache_get_or_fetch`).
    db: Db,
    /// Response-cache byte budget, forwarded to `Db::cache_get_or_fetch`. 0
    /// means no eviction cap.
    cache_max_size: u64,
    /// Response-cache per-entry TTL, forwarded to `Db::cache_get_or_fetch`.
    cache_ttl: Duration,
    /// Lazily-built REST clients, one per agent tag.
    http: Cache<serenity::http::Http>,
    /// Lazily-established gateway connections' shard managers, one per agent
    /// tag. Each connection's event loop runs in a background task.
    gateway: Cache<ShardManager>,
}

impl Client {
    /// Build a Discord client. **Infallible** and **synchronous** — no I/O
    /// happens here. Tokens + resources are resolved lazily, per agent, on the
    /// first call for that agent. `cache_max_size` / `cache_ttl` are the
    /// response cache's budget + per-entry TTL (same knobs as the X client).
    pub fn new(db: Db, cache_max_size: u64, cache_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                cache_max_size,
                cache_ttl,
                http: DashMap::new(),
                gateway: DashMap::new(),
            }),
        }
    }

    /// The shared persistence handle.
    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    /// Run a read through the shared response cache: on a hit, deserialize the
    /// stored JSON body into `T`; on a miss, run `fetch`, store its
    /// JSON-serialized result, and return it. Single-flight + TTL + LRU are
    /// handled by `Db::cache_get_or_fetch`. The serenity model `T` round-trips
    /// through JSON (it's modeled from Discord JSON). Reads use this; writes
    /// call `http()` directly and never touch the cache.
    async fn cached<T, F, Fut>(&self, key: [u8; 32], fetch: F) -> Result<T, Error>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, Error>>,
    {
        let bytes = self
            .inner
            .db
            .cache_get_or_fetch(&key, self.inner.cache_max_size, self.inner.cache_ttl, || async {
                let value = fetch().await?;
                Ok::<Vec<u8>, Error>(serde_json::to_vec(&value)?)
            })
            .await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// The bot's own Discord identity. Per-user cached (`get_current_user`).
    pub async fn get_current_user(&self, agent_tag: &str) -> Result<CurrentUser, Error> {
        let key = cache::user_key(agent_tag, "get_current_user", &[]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_current_user().await?)
        })
        .await
    }

    /// The guilds (servers) the bot is in. Per-user cached (visibility is
    /// per-bot).
    pub async fn get_guilds(&self, agent_tag: &str) -> Result<Vec<GuildInfo>, Error> {
        let key = cache::user_key(agent_tag, "get_guilds", &[]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_guilds(None, None).await?)
        })
        .await
    }

    /// A guild's channels. Per-user cached — channel visibility is
    /// permission-filtered per bot.
    pub async fn get_channels(
        &self,
        agent_tag: &str,
        guild: GuildId,
    ) -> Result<Vec<GuildChannel>, Error> {
        let key = cache::user_key(agent_tag, "get_channels", &[&guild.get().to_le_bytes()]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_channels(guild).await?)
        })
        .await
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
    /// **all** intents + `handler`, spawns its event loop in a background task,
    /// and caches the resulting `ShardManager`. Later calls return the cached
    /// handle immediately — **the `handler` from the first call sticks; later
    /// calls' args are ignored** (one connection per agent).
    ///
    /// Uses [`GatewayIntents::all`], which includes the three privileged
    /// intents (message content, members, presences) — the bot's dev-portal
    /// toggles for these must be on (the login wizard enables them) or the
    /// Gateway rejects the connection with "Disallowed intent(s)".
    pub async fn gateway<H: EventHandler + 'static>(
        &self,
        agent_tag: &str,
        handler: H,
    ) -> Result<Arc<ShardManager>, Error> {
        let cell = self.inner.gateway.entry(agent_tag.to_string()).or_default().clone();
        cell.get_or_try_init(|| async {
            let token = self.bot_token(agent_tag).await?;
            let mut client = serenity::Client::builder(&token, GatewayIntents::all())
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

    /// Stop and evict the agent's gateway connection, if one is cached:
    /// shut down its shards and remove the cache entry so a later
    /// [`Self::gateway`] / [`Self::gateway_raw`] reconnects fresh. No-op if the
    /// agent has no connection.
    pub async fn stop_gateway(&self, agent_tag: &str) {
        if let Some((_, cell)) = self.inner.gateway.remove(agent_tag) {
            if let Some(manager) = cell.get() {
                manager.shutdown_all().await;
            }
        }
    }

    /// Like [`Self::gateway`] but with a [`RawEventHandler`] — the handler
    /// receives the raw `serenity::all::Event` enum (which serializes to JSON)
    /// for **every** gateway event, rather than the per-event-type
    /// [`EventHandler`] callbacks. Shares the same per-agent gateway cache as
    /// [`Self::gateway`] (whichever is called first for an agent wins).
    pub async fn gateway_raw<H: RawEventHandler + 'static>(
        &self,
        agent_tag: &str,
        handler: H,
    ) -> Result<Arc<ShardManager>, Error> {
        let cell = self.inner.gateway.entry(agent_tag.to_string()).or_default().clone();
        cell.get_or_try_init(|| async {
            let token = self.bot_token(agent_tag).await?;
            let mut client = serenity::Client::builder(&token, GatewayIntents::all())
                .raw_event_handler(handler)
                .await?;
            let shard_manager = client.shard_manager.clone();
            tokio::spawn(async move {
                let _ = client.start().await;
            });
            Ok(shard_manager)
        })
        .await
        .cloned()
    }
}
