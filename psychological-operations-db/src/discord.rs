//! Per-agent Discord bot token storage (`discord_auth` table).
//!
//! Written by `agents login discord` once the wizard scrapes the bot token
//! from the Discord developer portal; later read by the agent's Discord
//! gateway/REST client. Keyed by agent tag (one bot per agent).

use sqlx::Row;

use crate::{Db, Error};

impl Db {
    /// Upsert the bot token for `agent_tag`.
    pub async fn discord_auth_set(&self, agent_tag: &str, bot_token: &str) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO discord_auth (agent_tag, bot_token, updated_at) \
             VALUES ($1, $2, now()) \
             ON CONFLICT (agent_tag) DO UPDATE SET \
              bot_token = excluded.bot_token, \
              updated_at = now()",
        )
        .bind(agent_tag)
        .bind(bot_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The stored bot token for `agent_tag`, if any.
    pub async fn discord_auth_get(&self, agent_tag: &str) -> Result<Option<String>, Error> {
        let row = sqlx::query("SELECT bot_token FROM discord_auth WHERE agent_tag = $1")
            .bind(agent_tag)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>("bot_token")))
    }

    /// Drop the stored bot token for `agent_tag` (re-login / reset).
    pub async fn discord_auth_delete(&self, agent_tag: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM discord_auth WHERE agent_tag = $1")
            .bind(agent_tag)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
