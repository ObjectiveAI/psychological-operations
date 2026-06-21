//! Per-agent Discord bot credentials storage (`discord_auth` table).
//!
//! Filled incrementally by `agents login discord` as the wizard scrapes the
//! developer portal: `client_id` + `public_key` on the General Information
//! page, then `bot_token` on the Bot page. `client_id`/`public_key` are
//! public (invite link / interaction verification); `bot_token` is the
//! secret gateway/REST credential. Keyed by agent tag (one bot per agent).

use sqlx::Row;

use crate::{Db, Error};

/// One agent's stored Discord bot credentials. Fields are `Option` because
/// the row is built across wizard steps.
#[derive(Debug, Clone)]
pub struct DiscordAuth {
    /// Application (client) id — public; builds the invite link.
    pub client_id: Option<String>,
    /// Interaction public key — public; verifies HTTP interaction webhooks.
    pub public_key: Option<String>,
    /// Bot token — secret; the gateway/REST credential.
    pub bot_token: Option<String>,
}

impl Db {
    /// Upsert the full credential set for `agent_tag` in one write. The
    /// wizard accumulates all three values in memory and calls this once,
    /// only when everything is in hand.
    pub async fn discord_auth_set_all(
        &self,
        agent_tag: &str,
        client_id: &str,
        public_key: &str,
        bot_token: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO discord_auth (agent_tag, client_id, public_key, bot_token, updated_at) \
             VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (agent_tag) DO UPDATE SET \
              client_id = excluded.client_id, \
              public_key = excluded.public_key, \
              bot_token = excluded.bot_token, \
              updated_at = now()",
        )
        .bind(agent_tag)
        .bind(client_id)
        .bind(public_key)
        .bind(bot_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The stored credentials for `agent_tag`, if a row exists.
    pub async fn discord_auth_get(&self, agent_tag: &str) -> Result<Option<DiscordAuth>, Error> {
        let row = sqlx::query(
            "SELECT client_id, public_key, bot_token FROM discord_auth WHERE agent_tag = $1",
        )
        .bind(agent_tag)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| DiscordAuth {
            client_id: r.get::<Option<String>, _>("client_id"),
            public_key: r.get::<Option<String>, _>("public_key"),
            bot_token: r.get::<Option<String>, _>("bot_token"),
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
