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
    Builder, ChannelId, ChannelType, CreateMessage, CreateThread, CurrentUser, EditMessage, Emoji,
    EventHandler, GatewayIntents, GuildChannel, GuildId, GuildInfo, Member, Message, MessageId,
    MessagePagination, RawEventHandler, ReactionType, Role, RoleId, ShardManager, User, UserId,
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

/// Discord's max page size for `GET /guilds/{id}/members`.
pub const MEMBERS_PAGE: u64 = 1000;

/// Discord's max page size for `GET /channels/{id}/messages`.
pub const MESSAGES_PAGE: u8 = 100;

/// Args for [`Client::get_guild_members`]. The page size is fixed internally
/// ([`MEMBERS_PAGE`]) so cache keys never carry a variable limit — page by
/// `after` (the last user id of the previous page).
#[derive(Debug, Clone, Copy)]
pub struct GetGuildMembers {
    pub guild: GuildId,
    pub after: Option<UserId>,
}

/// Args for [`Client::get_messages`]. Newest-first; the page size is fixed
/// internally ([`MESSAGES_PAGE`]) so cache keys carry only `(channel, before)`
/// — page by `before` (the oldest id of the previous page).
#[derive(Debug, Clone, Copy)]
pub struct GetMessages {
    pub channel: ChannelId,
    pub before: Option<MessageId>,
}

/// Discord's max page size for `GET /channels/{id}/messages/{id}/reactions/{e}`.
pub const REACTIONS_PAGE: u8 = 100;

/// Args for [`Client::get_reaction_users`]. The page size is fixed internally
/// ([`REACTIONS_PAGE`]) so cache keys carry only `(channel, message, emoji,
/// after)` — page by `after` (the last user id of the previous page).
#[derive(Debug, Clone)]
pub struct GetReactionUsers {
    pub channel: ChannelId,
    pub message: MessageId,
    pub emoji: ReactionType,
    pub after: Option<UserId>,
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

    /// The bot's application emojis (usable in any guild). Per-user cached —
    /// these belong to the calling bot's application.
    pub async fn get_application_emojis(&self, agent_tag: &str) -> Result<Vec<Emoji>, Error> {
        let key = cache::user_key(agent_tag, "get_application_emojis", &[]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_application_emojis().await?)
        })
        .await
    }

    /// A guild member. Global cached (member data is the same regardless of
    /// which bot fetched it).
    pub async fn get_member(
        &self,
        agent_tag: &str,
        guild: GuildId,
        user: UserId,
    ) -> Result<Member, Error> {
        let key = cache::global_key(
            "get_member",
            &[&guild.get().to_le_bytes(), &user.get().to_le_bytes()],
        );
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_member(guild, user).await?)
        })
        .await
    }

    /// A user by id (no guild context). Global cached.
    pub async fn get_user(&self, agent_tag: &str, user: UserId) -> Result<User, Error> {
        let key = cache::global_key("get_user", &[&user.get().to_le_bytes()]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_user(user).await?)
        })
        .await
    }

    /// A role in a guild. Global cached.
    pub async fn get_guild_role(
        &self,
        agent_tag: &str,
        guild: GuildId,
        role: RoleId,
    ) -> Result<Role, Error> {
        let key = cache::global_key(
            "get_guild_role",
            &[&guild.get().to_le_bytes(), &role.get().to_le_bytes()],
        );
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_guild_role(guild, role).await?)
        })
        .await
    }

    /// One page (up to [`MEMBERS_PAGE`]) of a guild's members, after the
    /// `after` cursor. Global cached. Callers loop, advancing `after` to the
    /// last returned member's id, until a short/empty page.
    pub async fn get_guild_members(
        &self,
        agent_tag: &str,
        req: GetGuildMembers,
    ) -> Result<Vec<Member>, Error> {
        let GetGuildMembers { guild, after } = req;
        let after_bytes = cache::opt_cursor(after.map(|u| u.get()));
        let key =
            cache::global_key("get_guild_members", &[&guild.get().to_le_bytes(), &after_bytes]);
        self.cached(key, || async move {
            let http = self.http(agent_tag).await?;
            Ok(http
                .get_guild_members(guild, Some(MEMBERS_PAGE), after.map(|u| u.get()))
                .await?)
        })
        .await
    }

    /// One page (up to [`MESSAGES_PAGE`]) of a channel's messages, newest
    /// first, older than the `before` cursor. Global cached. Callers loop,
    /// advancing `before` to the oldest returned id, until a short/empty page.
    pub async fn get_messages(
        &self,
        agent_tag: &str,
        req: GetMessages,
    ) -> Result<Vec<Message>, Error> {
        let GetMessages { channel, before } = req;
        let before_bytes = cache::opt_cursor(before.map(|m| m.get()));
        let key =
            cache::global_key("get_messages", &[&channel.get().to_le_bytes(), &before_bytes]);
        self.cached(key, || async move {
            let http = self.http(agent_tag).await?;
            let target = before.map(MessagePagination::Before);
            Ok(http.get_messages(channel, target, Some(MESSAGES_PAGE)).await?)
        })
        .await
    }

    /// A single message in full. Global cached.
    pub async fn get_message(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        message: MessageId,
    ) -> Result<Message, Error> {
        let key = cache::global_key(
            "get_message",
            &[&channel.get().to_le_bytes(), &message.get().to_le_bytes()],
        );
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_message(channel, message).await?)
        })
        .await
    }

    /// A guild's custom emojis. Global cached.
    pub async fn get_emojis(&self, agent_tag: &str, guild: GuildId) -> Result<Vec<Emoji>, Error> {
        let key = cache::global_key("get_emojis", &[&guild.get().to_le_bytes()]);
        self.cached(key, || async {
            let http = self.http(agent_tag).await?;
            Ok(http.get_emojis(guild).await?)
        })
        .await
    }

    /// One page (up to [`REACTIONS_PAGE`]) of the users who reacted to a message
    /// with a given emoji, after the `after` cursor. Global cached. Callers
    /// loop, advancing `after`, until a short/empty page.
    pub async fn get_reaction_users(
        &self,
        agent_tag: &str,
        req: GetReactionUsers,
    ) -> Result<Vec<User>, Error> {
        let GetReactionUsers {
            channel,
            message,
            emoji,
            after,
        } = req;
        let after_bytes = cache::opt_cursor(after.map(|u| u.get()));
        let key = cache::global_key(
            "get_reaction_users",
            &[
                &channel.get().to_le_bytes(),
                &message.get().to_le_bytes(),
                emoji.to_string().as_bytes(),
                &after_bytes,
            ],
        );
        self.cached(key, || async move {
            let http = self.http(agent_tag).await?;
            Ok(http
                .get_reaction_users(channel, message, &emoji, REACTIONS_PAGE, after.map(|u| u.get()))
                .await?)
        })
        .await
    }

    // ── writes (uncached; mutations never touch the cache) ──────────────────

    /// Validate the agent's bot token with a live `/users/@me` call —
    /// **uncached** (the auth gate must verify the token now, not a cache peek).
    pub async fn validate_token(&self, agent_tag: &str) -> Result<(), Error> {
        let http = self.http(agent_tag).await?;
        http.get_current_user().await?;
        Ok(())
    }

    /// Send a message to a channel, optionally as a reply. Returns the created
    /// message.
    pub async fn send_message(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        content: String,
        reply_to: Option<MessageId>,
    ) -> Result<Message, Error> {
        let http = self.http(agent_tag).await?;
        let mut builder = CreateMessage::new().content(content);
        if let Some(mid) = reply_to {
            // `(channel, message_id)` is a reply reference in this channel.
            builder = builder.reference_message((channel, mid));
        }
        Ok(builder.execute(&*http, (channel, None)).await?)
    }

    /// Open (or reuse) a DM with `user` and send a message there, optionally as
    /// a reply. Returns the created message (its `channel_id` is the DM).
    pub async fn send_direct_message(
        &self,
        agent_tag: &str,
        user: UserId,
        content: String,
        reply_to: Option<MessageId>,
    ) -> Result<Message, Error> {
        let http = self.http(agent_tag).await?;
        let dm = user.create_dm_channel(&*http).await?;
        let channel = dm.id;
        let mut builder = CreateMessage::new().content(content);
        if let Some(mid) = reply_to {
            builder = builder.reference_message((channel, mid));
        }
        Ok(builder.execute(&*http, (channel, None)).await?)
    }

    /// Replace the content of one of the bot's own messages.
    pub async fn edit_message(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        message: MessageId,
        content: String,
    ) -> Result<(), Error> {
        let http = self.http(agent_tag).await?;
        EditMessage::new()
            .content(content)
            .execute(&*http, (channel, message, None))
            .await?;
        Ok(())
    }

    /// Delete one of the bot's own messages.
    pub async fn delete_message(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        message: MessageId,
    ) -> Result<(), Error> {
        let http = self.http(agent_tag).await?;
        http.delete_message(channel, message, None).await?;
        Ok(())
    }

    /// Create a thread in `channel`. With `from_message`, start it from that
    /// message; without, a standalone public thread. Returns the thread channel.
    pub async fn create_thread(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        name: String,
        from_message: Option<MessageId>,
    ) -> Result<GuildChannel, Error> {
        let http = self.http(agent_tag).await?;
        let thread = match from_message {
            Some(message) => CreateThread::new(name).execute(&*http, (channel, Some(message))).await?,
            None => {
                CreateThread::new(name)
                    .kind(ChannelType::PublicThread)
                    .execute(&*http, (channel, None))
                    .await?
            }
        };
        Ok(thread)
    }

    /// Add the bot's reaction to a message.
    pub async fn add_reaction(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        message: MessageId,
        emoji: ReactionType,
    ) -> Result<(), Error> {
        let http = self.http(agent_tag).await?;
        http.create_reaction(channel, message, &emoji).await?;
        Ok(())
    }

    /// Remove the bot's own reaction from a message.
    pub async fn remove_reaction(
        &self,
        agent_tag: &str,
        channel: ChannelId,
        message: MessageId,
        emoji: ReactionType,
    ) -> Result<(), Error> {
        let http = self.http(agent_tag).await?;
        http.delete_reaction_me(channel, message, &emoji).await?;
        Ok(())
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
