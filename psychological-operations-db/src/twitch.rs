//! Twitch persistence: master app creds, per-agent OAuth tokens, the
//! per-agent channel-join set, and the rolling chat buffer.
//!
//! Unlike Discord/X, Twitch exposes no chat-history REST endpoint, so reads
//! are served from [`twitch_messages`](Db::twitch_messages_list) — a buffer
//! the daemon's IRC listener fills as messages arrive on the channels each
//! agent has JOINed ([`twitch_channels`](Db::twitch_channels_list)). This
//! module only persists/serves; the IRC loop and Helix client live elsewhere.

use sqlx::Row;

use crate::{unix_now, Db, Error};

/// Newest messages kept per `(agent_tag, channel_login)` in the buffer;
/// older rows are pruned on insert.
const MESSAGES_PER_CHANNEL_CAP: i64 = 500;

/// The registered Twitch application funding every agent's OAuth.
#[derive(Debug, Clone)]
pub struct TwitchApp {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: Option<String>,
}

/// One agent's stored Twitch user OAuth token + account identity.
#[derive(Debug, Clone)]
pub struct TwitchAuth {
    pub user_id: Option<String>,
    pub login: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    /// Unix seconds the access token expires; `None` if unknown.
    pub expires_at: Option<i64>,
}

/// One buffered chat message (a single joined-channel PRIVMSG).
#[derive(Debug, Clone)]
pub struct TwitchMessage {
    pub agent_tag: String,
    pub channel_login: String,
    pub message_id: String,
    pub user_id: String,
    pub user_login: String,
    pub content: String,
    /// Unix seconds the daemon received the message.
    pub sent_at: i64,
}

impl Db {
    // ── master app credentials ───────────────────────────────────────

    /// Upsert the master Twitch app credentials (keyed by `client_id`).
    pub async fn twitch_app_set(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        redirect_uri: Option<&str>,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO twitch_app_credentials (client_id, client_secret, redirect_uri) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (client_id) DO UPDATE SET \
                 client_secret = excluded.client_secret, \
                 redirect_uri = excluded.redirect_uri, \
                 saved_at = now()",
        )
        .bind(client_id)
        .bind(client_secret)
        .bind(redirect_uri)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The active Twitch app — the most-recently-saved row — or `None`.
    pub async fn twitch_app_active(&self) -> Result<Option<TwitchApp>, Error> {
        let row = sqlx::query(
            "SELECT client_id, client_secret, redirect_uri \
             FROM twitch_app_credentials ORDER BY saved_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| TwitchApp {
            client_id: r.get("client_id"),
            client_secret: r.get::<Option<String>, _>("client_secret"),
            redirect_uri: r.get::<Option<String>, _>("redirect_uri"),
        }))
    }

    /// Delete every stored Twitch app row (setup reset).
    pub async fn twitch_app_clear(&self) -> Result<(), Error> {
        sqlx::query("DELETE FROM twitch_app_credentials")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── per-agent auth ───────────────────────────────────────────────

    /// Upsert the full token set for `agent_tag` in one write.
    pub async fn twitch_auth_set(&self, agent_tag: &str, auth: &TwitchAuth) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO twitch_auth \
             (agent_tag, user_id, login, access_token, refresh_token, scope, expires_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
             ON CONFLICT (agent_tag) DO UPDATE SET \
                 user_id = excluded.user_id, \
                 login = excluded.login, \
                 access_token = excluded.access_token, \
                 refresh_token = excluded.refresh_token, \
                 scope = excluded.scope, \
                 expires_at = excluded.expires_at, \
                 updated_at = now()",
        )
        .bind(agent_tag)
        .bind(auth.user_id.as_deref())
        .bind(auth.login.as_deref())
        .bind(auth.access_token.as_deref())
        .bind(auth.refresh_token.as_deref())
        .bind(auth.scope.as_deref())
        .bind(auth.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The stored Twitch auth for `agent_tag`, if a row exists.
    pub async fn twitch_auth_get(&self, agent_tag: &str) -> Result<Option<TwitchAuth>, Error> {
        let row = sqlx::query(
            "SELECT user_id, login, access_token, refresh_token, scope, expires_at \
             FROM twitch_auth WHERE agent_tag = $1",
        )
        .bind(agent_tag)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| TwitchAuth {
            user_id: r.get::<Option<String>, _>("user_id"),
            login: r.get::<Option<String>, _>("login"),
            access_token: r.get::<Option<String>, _>("access_token"),
            refresh_token: r.get::<Option<String>, _>("refresh_token"),
            scope: r.get::<Option<String>, _>("scope"),
            expires_at: r.get::<Option<i64>, _>("expires_at"),
        }))
    }

    /// Drop the stored Twitch auth for `agent_tag` (re-login / reset).
    pub async fn twitch_auth_delete(&self, agent_tag: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM twitch_auth WHERE agent_tag = $1")
            .bind(agent_tag)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── channel-join set ─────────────────────────────────────────────

    /// Add a channel to `agent_tag`'s JOIN set (idempotent).
    pub async fn twitch_channels_add(
        &self,
        agent_tag: &str,
        channel_login: &str,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO twitch_channels (agent_tag, channel_login, added_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (agent_tag, channel_login) DO NOTHING",
        )
        .bind(agent_tag)
        .bind(channel_login)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a channel from `agent_tag`'s JOIN set.
    pub async fn twitch_channels_remove(
        &self,
        agent_tag: &str,
        channel_login: &str,
    ) -> Result<(), Error> {
        sqlx::query("DELETE FROM twitch_channels WHERE agent_tag = $1 AND channel_login = $2")
            .bind(agent_tag)
            .bind(channel_login)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Channels `agent_tag` has JOINed, alphabetical.
    pub async fn twitch_channels_list(&self, agent_tag: &str) -> Result<Vec<String>, Error> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT channel_login FROM twitch_channels WHERE agent_tag = $1 ORDER BY channel_login ASC",
        )
        .bind(agent_tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Every `(agent_tag, channel_login)` pair — the daemon's full JOIN map.
    pub async fn twitch_channels_all(&self) -> Result<Vec<(String, String)>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, channel_login FROM twitch_channels \
             ORDER BY agent_tag ASC, channel_login ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("agent_tag"), r.get("channel_login")))
            .collect())
    }

    /// Agents the Twitch daemon should listen for: those with a stored access
    /// token AND at least one JOINed channel. Mirrors `discord_daemon_agents`.
    pub async fn twitch_daemon_agents(&self) -> Result<Vec<String>, Error> {
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT c.agent_tag FROM twitch_channels c \
             JOIN twitch_auth a ON a.agent_tag = c.agent_tag \
             WHERE a.access_token IS NOT NULL \
             ORDER BY c.agent_tag ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(tags)
    }

    // ── chat buffer ──────────────────────────────────────────────────

    /// Insert one received message and prune the channel's buffer to the
    /// newest [`MESSAGES_PER_CHANNEL_CAP`]. Idempotent on `message_id`.
    pub async fn twitch_messages_insert(&self, msg: &TwitchMessage) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO twitch_messages \
             (agent_tag, channel_login, message_id, user_id, user_login, content, sent_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (agent_tag, channel_login, message_id) DO NOTHING",
        )
        .bind(&msg.agent_tag)
        .bind(&msg.channel_login)
        .bind(&msg.message_id)
        .bind(&msg.user_id)
        .bind(&msg.user_login)
        .bind(&msg.content)
        .bind(msg.sent_at)
        .execute(&mut *tx)
        .await?;

        // Prune everything older than the newest N for this (agent, channel).
        sqlx::query(
            "DELETE FROM twitch_messages \
             WHERE agent_tag = $1 AND channel_login = $2 AND message_id NOT IN ( \
                 SELECT message_id FROM twitch_messages \
                 WHERE agent_tag = $1 AND channel_login = $2 \
                 ORDER BY sent_at DESC LIMIT $3 )",
        )
        .bind(&msg.agent_tag)
        .bind(&msg.channel_login)
        .bind(MESSAGES_PER_CHANNEL_CAP)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// A window of buffered messages for `(agent_tag, channel_login)`, newest
    /// first, `offset`..`offset+count`.
    pub async fn twitch_messages_list(
        &self,
        agent_tag: &str,
        channel_login: &str,
        count: i64,
        offset: i64,
    ) -> Result<Vec<TwitchMessage>, Error> {
        let rows = sqlx::query(
            "SELECT agent_tag, channel_login, message_id, user_id, user_login, content, sent_at \
             FROM twitch_messages \
             WHERE agent_tag = $1 AND channel_login = $2 \
             ORDER BY sent_at DESC \
             LIMIT $3 OFFSET $4",
        )
        .bind(agent_tag)
        .bind(channel_login)
        .bind(count)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TwitchMessage {
                agent_tag: r.get("agent_tag"),
                channel_login: r.get("channel_login"),
                message_id: r.get("message_id"),
                user_id: r.get("user_id"),
                user_login: r.get("user_login"),
                content: r.get("content"),
                sent_at: r.get("sent_at"),
            })
            .collect())
    }

    /// Count of buffered messages for `(agent_tag, channel_login)`.
    pub async fn twitch_messages_count(
        &self,
        agent_tag: &str,
        channel_login: &str,
    ) -> Result<i64, Error> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM twitch_messages WHERE agent_tag = $1 AND channel_login = $2",
        )
        .bind(agent_tag)
        .bind(channel_login)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
}
