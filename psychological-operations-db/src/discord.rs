//! Per-agent Discord bot credentials storage (`discord_auth` table).
//!
//! Written by `agents login discord` once the wizard scrapes the
//! application's client id + bot token from the Discord developer portal;
//! later read by the agent's Discord gateway/REST client. The `client_id`
//! (application id) is public — it builds the bot's invite link; the
//! `bot_token` is secret. Keyed by agent tag (one bot per agent).

use sqlx::Row;

use crate::{Db, Error};

/// One agent's stored Discord bot credentials.
#[derive(Debug, Clone)]
pub struct DiscordAuth {
    /// Application (client) id — public; used to build the invite link.
    pub client_id: String,
    /// Bot token — secret; the gateway/REST credential.
    pub bot_token: String,
}

impl Db {
    /// Upsert the client id + bot token for `agent_tag`.
    pub async fn discord_auth_set(
        &self,
        agent_tag: &str,
        client_id: &str,
        bot_token: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO discord_auth (agent_tag, client_id, bot_token, updated_at) \
             VALUES ($1, $2, $3, now()) \
             ON CONFLICT (agent_tag) DO UPDATE SET \
              client_id = excluded.client_id, \
              bot_token = excluded.bot_token, \
              updated_at = now()",
        )
        .bind(agent_tag)
        .bind(client_id)
        .bind(bot_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The stored credentials for `agent_tag`, if any.
    pub async fn discord_auth_get(&self, agent_tag: &str) -> Result<Option<DiscordAuth>, Error> {
        let row = sqlx::query("SELECT client_id, bot_token FROM discord_auth WHERE agent_tag = $1")
            .bind(agent_tag)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| DiscordAuth {
            client_id: r.get::<String, _>("client_id"),
            bot_token: r.get::<String, _>("bot_token"),
        }))
    }

    /// Drop the stored credentials for `agent_tag` (re-login / reset).
    pub async fn discord_auth_delete(&self, agent_tag: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM discord_auth WHERE agent_tag = $1")
            .bind(agent_tag)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
