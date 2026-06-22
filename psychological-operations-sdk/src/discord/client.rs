//! Discord API client (serenity-backed).
//!
//! Two capabilities, both authenticated per-agent from the database:
//!
//! - **REST** ([`Client::http`]) — a `serenity::http::Http` for regular API
//!   calls (read channel history, send a message, open a DM, …).
//! - **Gateway** ([`Client::gateway`]) — a built `serenity::Client` wired with
//!   gateway intents + an [`EventHandler`] for live event listening. The caller
//!   drives it (`.start().await`).
//!
//! Like [`crate::x::client::Client`], this carries **no identity** — the bot
//! token is resolved from the DB on every call, so a re-login (or token reset)
//! is picked up without rebuilding the client. Discord is much simpler than X,
//! though: the bot token is a static secret read directly from `discord_auth`
//! by agent tag, with no twid indirection and no OAuth refresh/lock dance.

use psychological_operations_db::Db;
use serenity::all::{EventHandler, GatewayIntents};

use super::error::Error;

/// Discord client. See module docs.
#[derive(Debug, Clone)]
pub struct Client {
    /// The single persistence layer — holds each agent's `discord_auth` row.
    /// Cheap to clone (the pool is `Arc` internally).
    db: Db,
}

impl Client {
    /// Build a Discord client. **Infallible** and **synchronous** — no I/O
    /// happens here (matches [`crate::x::client::Client::new`]). The client
    /// carries no identity; every call takes an `agent_tag` and resolves that
    /// agent's bot token from the DB.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The shared persistence handle.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Resolve the agent's bot token from the DB. `discord_auth_get(tag)` then
    /// the row's `bot_token`; [`Error::NotAuthed`] if there's no row or no
    /// token.
    async fn bot_token(&self, agent_tag: &str) -> Result<String, Error> {
        self.db
            .discord_auth_get(agent_tag)
            .await?
            .and_then(|a| a.bot_token)
            .ok_or_else(|| Error::NotAuthed(agent_tag.to_string()))
    }

    /// A serenity REST client authed as the agent's bot. Use for regular API
    /// calls (e.g. `GET /channels/{id}/messages`, send a message, open a DM).
    pub async fn http(&self, agent_tag: &str) -> Result<serenity::http::Http, Error> {
        let token = self.bot_token(agent_tag).await?;
        Ok(serenity::http::Http::new(&token))
    }

    /// Build a gateway client authed as the agent's bot, with the given
    /// `intents` + `handler`. Returns the built `serenity::Client`; the caller
    /// drives it (`.start().await`, or shard control). Keeping build and run
    /// separate lets the caller own the run loop + shutdown.
    pub async fn gateway<H: EventHandler + 'static>(
        &self,
        agent_tag: &str,
        intents: GatewayIntents,
        handler: H,
    ) -> Result<serenity::Client, Error> {
        let token = self.bot_token(agent_tag).await?;
        serenity::Client::builder(&token, intents)
            .event_handler(handler)
            .await
            .map_err(Error::Serenity)
    }
}
