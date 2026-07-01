//! Twitch Helix API client (reqwest-backed).
//!
//! Multi-agent: every call takes an `agent_tag` and authenticates from that
//! agent's `twitch_auth` row (the OAuth access token) plus the active Twitch
//! app's `client_id` (the `Client-Id` header Helix requires). Two capabilities:
//!
//! - **Helix REST** — typed per-endpoint methods (`get_user_by_login`,
//!   `send_message`). Reads are fronted by the shared response cache
//!   (`Db::cache_get_or_fetch`: single-flight lock, TTL, LRU — same backend as
//!   the X/Discord clients); writes go straight through.
//! - **OAuth validate** ([`Client::validate`]) — the token liveness / whoami
//!   check against `id.twitch.tv`. Always uncached (the auth gate must verify
//!   the token *now*, not a cache peek).
//!
//! Unlike Discord, there's no per-agent resource to build + cache — the token
//! and client id are resolved per call from the db (so a re-login is picked up
//! on the next call), and the single shared `reqwest::Client` is used for
//! everything. Cloning the `Client` shares the same inner state (`Arc`).

use std::sync::Arc;
use std::time::Duration;

use psychological_operations_db::Db;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::cache;
use super::error::Error;

/// OAuth base — token validation (whoami / liveness).
const OAUTH_BASE: &str = "https://id.twitch.tv";

/// Helix API base — user lookup + chat send.
const HELIX_BASE: &str = "https://api.twitch.tv/helix";

/// Twitch client. Cheap to clone — clones share the inner state.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    /// The single persistence layer — holds each agent's `twitch_auth` row and
    /// the active `twitch_app` credentials, and backs the response cache
    /// (`cache_get_or_fetch`).
    db: Db,
    /// Response-cache byte budget, forwarded to `Db::cache_get_or_fetch`. 0
    /// means no eviction cap.
    cache_max_size: u64,
    /// Response-cache per-entry TTL, forwarded to `Db::cache_get_or_fetch`.
    cache_ttl: Duration,
    /// The shared HTTP client used for every Helix / OAuth call.
    http: reqwest::Client,
}

/// The result of an OAuth token validation — the identity the token acts as.
/// (Twitch returns more — `client_id`, `expires_in` — which we ignore.)
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ValidateResponse {
    pub login: String,
    pub user_id: String,
    pub scopes: Vec<String>,
}

/// A Twitch user as returned by Helix `GET /users`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct HelixUser {
    pub id: String,
    pub login: String,
    pub display_name: String,
}

/// The outcome of a Helix `POST /chat/messages`. `is_sent` false means Twitch
/// dropped the message (the reason is surfaced as an [`Error::Http`]).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct SentMessage {
    pub message_id: String,
    pub is_sent: bool,
}

/// Helix wraps list results in `{ "data": [...] }`.
#[derive(serde::Deserialize)]
struct HelixData<T> {
    data: Vec<T>,
}

/// One entry in the `POST /chat/messages` response `data` array.
#[derive(serde::Deserialize)]
struct SendMessageResult {
    message_id: String,
    is_sent: bool,
    #[serde(default)]
    drop_reason: Option<DropReason>,
}

/// Why Twitch dropped a chat message (present when `is_sent` is false).
#[derive(serde::Deserialize)]
struct DropReason {
    #[allow(dead_code)]
    code: Option<String>,
    message: String,
}

impl Client {
    /// Build a Twitch client. **Infallible** and **synchronous** — no I/O
    /// happens here. Tokens + the app client id are resolved lazily, per call,
    /// from the db. `cache_max_size` / `cache_ttl` are the response cache's
    /// budget + per-entry TTL (same knobs as the X/Discord clients).
    pub fn new(db: Db, cache_max_size: u64, cache_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                cache_max_size,
                cache_ttl,
                http: reqwest::Client::new(),
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
    /// handled by `Db::cache_get_or_fetch`. Reads use this; writes call the
    /// HTTP client directly and never touch the cache.
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

    // ── reads ────────────────────────────────────────────────────────────

    /// Validate the agent's OAuth token with a live `GET /oauth2/validate` —
    /// **uncached** (the auth gate must verify the token now, not a cache
    /// peek). Doubles as whoami: the response carries the token's `login` +
    /// `user_id`. Authenticated with the literal `Authorization: OAuth <token>`
    /// scheme (Twitch's validate endpoint, unlike Helix, wants "OAuth", not
    /// "Bearer").
    pub async fn validate(&self, agent_tag: &str) -> Result<ValidateResponse, Error> {
        let token = self.token(agent_tag).await?;
        let resp = self
            .inner
            .http
            .get(format!("{OAUTH_BASE}/oauth2/validate"))
            .header("Authorization", format!("OAuth {token}"))
            .send()
            .await?;
        self.json_ok(resp).await
    }

    /// Look up a Twitch user by their login (channel) name via Helix
    /// `GET /users?login=<login>`. Returns the first match, or `None` if no
    /// such user. Cached.
    pub async fn get_user_by_login(
        &self,
        agent_tag: &str,
        login: &str,
    ) -> Result<Option<HelixUser>, Error> {
        let key = cache::user_key(agent_tag, "get_user_by_login", &[login.as_bytes()]);
        let users: Vec<HelixUser> = self
            .cached(key, || async {
                let token = self.token(agent_tag).await?;
                let client_id = self.client_id().await?;
                let resp = self
                    .inner
                    .http
                    .get(format!("{HELIX_BASE}/users"))
                    .query(&[("login", login)])
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Client-Id", client_id)
                    .send()
                    .await?;
                let body: HelixData<HelixUser> = self.json_ok(resp).await?;
                Ok(body.data)
            })
            .await?;
        Ok(users.into_iter().next())
    }

    // ── writes (uncached; mutations never touch the cache) ────────────────

    /// Send a chat message to a channel via Helix `POST /chat/messages`,
    /// optionally as a reply to `reply_parent_message_id`. `broadcaster_id` is
    /// the target channel's user id; `sender_id` is the agent's OWN Twitch user
    /// id (the caller resolves it from the agent's `twitch_auth`). If Twitch
    /// accepts but drops the message (`is_sent` false), the `drop_reason` is
    /// surfaced as [`Error::Http`].
    pub async fn send_message(
        &self,
        agent_tag: &str,
        broadcaster_id: &str,
        sender_id: &str,
        message: &str,
        reply_parent_message_id: Option<&str>,
    ) -> Result<SentMessage, Error> {
        let token = self.token(agent_tag).await?;
        let client_id = self.client_id().await?;
        let mut body = serde_json::json!({
            "broadcaster_id": broadcaster_id,
            "sender_id": sender_id,
            "message": message,
        });
        if let Some(parent) = reply_parent_message_id {
            body["reply_parent_message_id"] = serde_json::Value::String(parent.to_string());
        }
        let resp = self
            .inner
            .http
            .post(format!("{HELIX_BASE}/chat/messages"))
            .header("Authorization", format!("Bearer {token}"))
            .header("Client-Id", client_id)
            .json(&body)
            .send()
            .await?;
        let parsed: HelixData<SendMessageResult> = self.json_ok(resp).await?;
        let first = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Http("twitch returned no send result".to_string()))?;
        if !first.is_sent {
            let reason = first
                .drop_reason
                .map(|d| d.message)
                .unwrap_or_else(|| "message was dropped (no reason given)".to_string());
            return Err(Error::Http(format!("message not sent: {reason}")));
        }
        Ok(SentMessage {
            message_id: first.message_id,
            is_sent: first.is_sent,
        })
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Check the status, then decode the body into `T`. A non-2xx status is the
    /// authorized request's own outcome ([`Error::Http`], agent-facing); a
    /// decode failure is our model being wrong ([`Error::Serde`], system).
    async fn json_ok<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T, Error> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Http(format!("twitch api {status}: {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Resolve the agent's Twitch OAuth access token from the db.
    /// `twitch_auth_get(tag)` then the row's `access_token`; [`Error::NotAuthed`]
    /// if there's no row or no token.
    async fn token(&self, agent_tag: &str) -> Result<String, Error> {
        self.inner
            .db
            .twitch_auth_get(agent_tag)
            .await?
            .and_then(|a| a.access_token)
            .ok_or_else(|| {
                Error::NotAuthed(format!(
                    "agent '{agent_tag}' has no Twitch access token — run `agents login twitch` first"
                ))
            })
    }

    /// The active Twitch app's `client_id` — the `Client-Id` header every Helix
    /// call requires. [`Error::NotAuthed`] if no app is configured.
    async fn client_id(&self) -> Result<String, Error> {
        self.inner
            .db
            .twitch_app_active()
            .await?
            .map(|a| a.client_id)
            .ok_or_else(|| {
                Error::NotAuthed(
                    "no active Twitch app configured — set the Twitch app credentials first"
                        .to_string(),
                )
            })
    }
}
