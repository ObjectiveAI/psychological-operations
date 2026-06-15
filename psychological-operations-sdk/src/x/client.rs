//! X v2 API client.
//!
//! Single constructor [`Client::new`] — **infallible** and
//! **synchronous**. No I/O happens at construction time: persistence
//! goes through the shared [`Db`] handle (postgres pool), and the wire
//! bearer is resolved per request by reading the auth state referenced
//! by [`Client::auth_mode`] ("on the fly"). Live changes to the
//! signed-in X-App or persona (browser sign-out / sign-in) are picked
//! up on the next request without rebuilding the Client.
//!
//! Auth ownership lives here too: [`Client::read_auth`] is a cheap
//! lockless read, [`Client::lock_auth`] acquires the db's two-tier
//! (in-process tokio mutex + postgres advisory) lock, and
//! [`Client::write_auth`] consumes the lock and writes the
//! `auth_tokens` row. The [`super::auth::AuthLock`] returned by
//! `lock_auth` cannot be constructed externally — `lock_auth` is its
//! only producer.
//!
//! The codegen'd per-endpoint helpers in
//! `crate::x::*::{get,post,put,delete}::http` route through the
//! `pub(crate)` `send_*` family on this type — external callers
//! drive everything via the codegen helpers, never the generic
//! methods.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client as ReqwestClient, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use psychological_operations_db::{Db, signed_in_x_user_id};

use super::{AuthError, Error};
use super::auth::{self, AuthLock, PersonaKey};
use super::cache::{request_key, request_key_auth_scoped};
use crate::browser::auth_json::{self, PersonaKind, Tokens};
use crate::browser::mode::Mode;
use crate::x::types::Problem;

/// Default base URL for the X v2 API. Inlined where used —
/// no caller currently overrides it.
pub const DEFAULT_BASE_URL: &str = "https://api.x.com/2";

/// What kind of credentials this Client uses on the wire.
///
/// Resolution happens per-request, not at construction —
/// changing the signed-in X-App or persona at runtime is
/// reflected on the next call without rebuilding the Client.
///
/// * `XApp` — durable App-only Bearer from `x_app.json`. For
///   read-only endpoints; never refreshed.
/// * `Psyop(name)` — per-persona OAuth tokens stored in
///   `<config>/.../browser/psyop/<name>/handles/<persona_twid>/<x_app_twid>/auth.json`.
///   Refreshed via X's token endpoint when expiring within
///   [`auth_json::FRESHNESS_BUFFER`].
/// * `Agent(name)` — same shape, rooted under
///   `browser/agent/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuthMode {
    XApp,
    Psyop(String),
    Agent(String),
}

/// X v2 API client. See module docs.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) client: ReqwestClient,
    /// When true, every `send*` short-circuits to
    /// `crate::x::mock::*` instead of hitting the real X API,
    /// and `lock_auth`/`write_auth` reject with an error.
    pub(crate) mock: bool,
    /// Bytes — cache size budget. 0 means "no eviction cap"
    /// (entries still get stored; the LRU loop is skipped).
    pub(crate) cache_max_size: u64,
    /// Plumbed but unused today — see [`Cache.cache_ttl`].
    pub(crate) cache_ttl: Duration,
    /// Root of ALL on-disk state (the value of `OBJECTIVEAI_STATE_DIR`
    /// upstream). Only the CEF profile tree (browser cookies) still
    /// lives here; everything else moved to postgres. Assumed to
    /// already exist.
    pub(crate) state_dir: Arc<PathBuf>,
    /// The single persistence layer — response cache, advisory locker,
    /// queue, engagement, auth tokens, x_app config. Cheap to clone
    /// (the pool is `Arc` internally). Quota is no longer enforced here
    /// — it moved to the MCP, per tool call.
    pub(crate) db: Db,
}

impl Client {
    /// Build an X v2 API client. **Infallible** and
    /// **synchronous** — no I/O happens here. The Client carries no
    /// identity; every request takes an `auth: &AuthMode` argument and
    /// resolves the bearer for that persona on the fly.
    pub fn new(
        client: ReqwestClient,
        mock: bool,
        cache_max_size: u64,
        cache_ttl: Duration,
        state_dir: PathBuf,
        db: Db,
    ) -> Self {
        Self {
            client,
            mock,
            cache_max_size,
            cache_ttl,
            state_dir: Arc::new(state_dir),
            db,
        }
    }

    /// The shared persistence handle. Callers reach the queue /
    /// engagement / auth surfaces through `client.db().*`.
    pub fn db(&self) -> &Db {
        &self.db
    }

    // ===================================================================
    // Auth-file surface (read / lock / write).
    //
    // The persona is derived from the `auth` argument + CEF cookies on
    // every call — no method takes a `PersonaKey` argument.
    // `AuthMode::XApp` can't use these methods (no persona to bind);
    // they return an error.
    // ===================================================================

    /// Read the stored tokens for the persona `auth` resolves to. No
    /// locking. Returns `Ok(None)` if none exist. Errors for
    /// `AuthMode::XApp` (no persona).
    pub async fn read_auth(&self, auth: &AuthMode) -> Result<Option<Tokens>, Error> {
        let persona = self.resolve_persona(auth).await.map_err(Error::Authorization)?;
        self.read_auth_at(&persona).await.map_err(Error::Authorization)
    }

    /// Acquire the two-tier auth lock for the persona `auth` resolves
    /// to. Returned `AuthLock` is consumed by [`Self::write_auth`] (or
    /// dropped for best-effort release without writing). The lock
    /// cannot be constructed externally — `lock_auth` is the only
    /// producer. Errors for `AuthMode::XApp`.
    pub async fn lock_auth(&self, auth: &AuthMode) -> Result<AuthLock, Error> {
        if self.mock {
            return Err(Error::Other(
                "lock_auth not supported in mock mode".into(),
            ));
        }
        let persona = self.resolve_persona(auth).await.map_err(Error::Authorization)?;
        self.lock_auth_at(&persona).await.map_err(Error::Authorization)
    }

    /// Crate-internal — explicit persona for code paths that
    /// already resolved one (e.g. `persona_bearer` reusing the
    /// persona it just resolved instead of re-running the cookies
    /// dance).
    async fn read_auth_at(&self, persona: &PersonaKey) -> Result<Option<Tokens>, AuthError> {
        let value = self
            .db
            .auth_get(
                persona.kind.db_kind(),
                &persona.name,
                &persona.persona_twid,
                &persona.x_app_twid,
            )
            .await
            .map_err(AuthError::Store)?;
        match value {
            Some(v) => {
                let tokens: Tokens = serde_json::from_value(v)
                    .map_err(|e| AuthError::TokenSerde(e.to_string()))?;
                Ok(Some(tokens))
            }
            None => Ok(None),
        }
    }

    async fn lock_auth_at(&self, persona: &PersonaKey) -> Result<AuthLock, AuthError> {
        let key = auth::auth_lock_key(persona);
        let guard = self.db.lock(&key).await.map_err(AuthError::Store)?;
        Ok(AuthLock::new(guard, persona.clone()))
    }

    /// Write `new_data` to the persona's `auth_tokens` row, then
    /// release the advisory lock (awaited).
    pub async fn write_auth(
        &self,
        lock: AuthLock,
        new_data: &Tokens,
    ) -> Result<(), Error> {
        if self.mock {
            return Err(Error::Other(
                "write_auth not supported in mock mode".into(),
            ));
        }
        self.write_auth_inner(lock, new_data).await.map_err(Error::Authorization)
    }

    /// Low-level token write (no mock guard) returning the typed
    /// [`AuthError`]. Shared by the public [`Self::write_auth`] and the
    /// in-line refresh in [`Self::persona_bearer`].
    async fn write_auth_inner(
        &self,
        lock: AuthLock,
        new_data: &Tokens,
    ) -> Result<(), AuthError> {
        let persona = lock.persona();
        let value = serde_json::to_value(new_data)
            .map_err(|e| AuthError::TokenSerde(e.to_string()))?;
        self.db
            .auth_set(
                persona.kind.db_kind(),
                &persona.name,
                &persona.persona_twid,
                &persona.x_app_twid,
                &value,
            )
            .await
            .map_err(AuthError::Store)?;
        lock.guard.release().await;
        Ok(())
    }

    // ===================================================================
    // Per-call auth resolution.
    // ===================================================================

    /// Resolve the current Bearer for `auth` — reads the X-App config
    /// for `XApp`, runs the read/lock/double-check/refresh dance for
    /// `Psyop`/`Agent`. All I/O happens here on every call.
    async fn current_bearer_token(&self, auth: &AuthMode) -> Result<String, AuthError> {
        match auth {
            AuthMode::XApp => {
                let x_app = super::x_app::config::load(&self.db)
                    .await
                    .map_err(|e| AuthError::XAppNotConfigured(e.to_string()))?;
                x_app.bearer_token.ok_or_else(|| {
                    AuthError::XAppNotConfigured(
                        "no bearer_token — re-run `psychological-operations x_app setup`".into(),
                    )
                })
            }
            AuthMode::Psyop(_) | AuthMode::Agent(_) => self.persona_bearer(auth).await,
        }
    }

    /// The authenticated Twitter user-id (twid) for `auth`. For `XApp`
    /// it's the X-App account's twid; for `Psyop` / `Agent` it's the
    /// persona's twid resolved via [`resolve_persona`]. Used to fold an
    /// auth identity into the cache key for endpoints whose response
    /// varies by authed user (today: `/2/users/me`).
    pub(crate) async fn current_twid(&self, auth: &AuthMode) -> Result<String, AuthError> {
        match auth {
            AuthMode::XApp => signed_in_x_user_id(
                self.state_dir.as_ref(),
                &Mode::XApp.cache_subdir(),
            )
            .await
            .map_err(|e| AuthError::Cookie(format!("x-app twid: {e}")))?
            .ok_or_else(|| AuthError::NotSignedIn("X-App account".into())),
            AuthMode::Psyop(_) | AuthMode::Agent(_) => {
                Ok(self.resolve_persona(auth).await?.persona_twid)
            }
        }
    }

    /// Resolve the persona from `auth` + cookies. Errors for
    /// `AuthMode::XApp` and when no persona / X-App is signed in to the
    /// matching CEF profile. Reused by `read_auth`, `lock_auth`, and
    /// `persona_bearer`.
    async fn resolve_persona(&self, auth: &AuthMode) -> Result<PersonaKey, AuthError> {
        let (kind, name) = match auth {
            AuthMode::XApp => {
                return Err(AuthError::Unsupported(
                    "auth file methods are not available for AuthMode::XApp — \
                     XApp credentials live in x_app.json, not auth.json".into(),
                ));
            }
            AuthMode::Psyop(name) => (PersonaKind::Psyop, name.clone()),
            AuthMode::Agent(name) => (PersonaKind::Agent, name.clone()),
        };
        let cookie_mode = match kind {
            PersonaKind::Psyop => Mode::PsyopAuthorize { name: name.clone() },
            PersonaKind::Agent => Mode::AgentAuthorize { name: name.clone() },
        };
        let persona_twid = signed_in_x_user_id(
            &self.state_dir,
            &cookie_mode.cache_subdir(),
        )
        .await
        .map_err(|e| AuthError::Cookie(format!("persona: {e}")))?
        .ok_or_else(|| AuthError::NotSignedIn(format!("{kind:?} '{name}'")))?;
        let x_app_twid = signed_in_x_user_id(
            &self.state_dir,
            &Mode::XApp.cache_subdir(),
        )
        .await
        .map_err(|e| AuthError::Cookie(format!("x-app: {e}")))?
        .ok_or_else(|| AuthError::NotSignedIn("X-App account".into()))?;
        Ok(PersonaKey { kind, name, persona_twid, x_app_twid })
    }

    /// Resolve persona, read auth.json, refresh through the two-
    /// tier lock if stale. Shared by `Psyop` / `Agent` variants
    /// of [`current_bearer_token`].
    async fn persona_bearer(&self, auth: &AuthMode) -> Result<String, AuthError> {
        let persona = self.resolve_persona(auth).await?;

        // 1. Cheap, lockless read.
        if let Some(t) = self.read_auth_at(&persona).await? {
            if auth_json::is_fresh(&t) {
                return Ok(t.access_token);
            }
        }
        // 2. Stale or missing — acquire the two-tier lock.
        let lock = self.lock_auth_at(&persona).await?;
        // 3. Re-read after the lock — someone else may have refreshed.
        let stale = match self.read_auth_at(&persona).await? {
            Some(t) if auth_json::is_fresh(&t) => {
                drop(lock);
                return Ok(t.access_token);
            }
            Some(t) => t,
            None => {
                drop(lock);
                return Err(AuthError::NoTokens(persona.name.clone()));
            }
        };
        let refresh_token = stale
            .refresh_token
            .as_deref()
            .ok_or(AuthError::NoRefreshToken)?;
        // 4. Refresh via X's OAuth token endpoint.
        let x_app = super::x_app::config::load(&self.db)
            .await
            .map_err(|e| AuthError::XAppNotConfigured(e.to_string()))?;
        if !x_app.is_complete() {
            return Err(AuthError::XAppNotConfigured(
                "client_id/client_secret missing — run `x_app setup`".into(),
            ));
        }
        let client_id = x_app.client_id.expect("is_complete guarantees client_id");
        let client_secret = x_app
            .client_secret
            .expect("is_complete guarantees client_secret");
        let new_tokens = super::oauth::tokens::refresh(
            &client_id,
            &client_secret,
            refresh_token,
        )
        .await
        .map_err(|e| AuthError::Refresh(e.to_string()))?;
        let access = new_tokens.access_token.clone();
        self.write_auth_inner(lock, &new_tokens).await?;
        Ok(access)
    }

    // ===================================================================
    // Generic send / fetch surface (pub(crate); codegen-driven).
    // ===================================================================

    /// Build an authorized `RequestBuilder` for `path`. `pub(crate)`
    /// — only the codegen helpers + the SDK's own send methods
    /// should construct requests.
    pub(crate) async fn request(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let token = self.current_bearer_token(auth).await.map_err(Error::Authorization)?;
        let url = format!(
            "{}/{}",
            DEFAULT_BASE_URL.trim_end_matches('/'),
            path.trim_start_matches('/'),
        );
        let bare = token.strip_prefix("Bearer ").unwrap_or(token.as_str()).to_string();
        Ok(self
            .client
            .request(method, &url)
            .header("authorization", format!("Bearer {bare}")))
    }

    /// GET `path` with `query` URL-encoded.
    pub(crate) async fn send_with_query<T, Q>(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
        query: &Q,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send_with_query(method, path, query);
        }
        let rb = self.request(auth, method, path).await?.query(query);
        let raw = self.execute_cached(Some(auth), rb, cache, auth_scoped).await?;
        decode_body(&raw)
    }

    /// Send `method` to `path` with an optional JSON body.
    pub(crate) async fn send<T, B>(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
        body: Option<&B>,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send(method, path, body);
        }
        let mut rb = self.request(auth, method, path).await?;
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let raw = self.execute_cached(Some(auth), rb, cache, auth_scoped).await?;
        decode_body(&raw)
    }

    /// Like `send_with_query` but discards the response body.
    pub(crate) async fn send_with_query_no_response<Q>(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
        query: &Q,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<(), Error>
    where
        Q: Serialize + ?Sized,
    {
        let _ = (cache, auth_scoped);
        if self.mock {
            return crate::x::mock::send_with_query_no_response(method, path, query);
        }
        let req = self
            .request(auth, method, path)
            .await?
            .query(query)
            .build()
            .map_err(Error::RequestBuild)?;
        let response = self.client.execute(req).await.map_err(Error::Transport)?;
        let code = response.status();
        if code.is_success() {
            return Ok(());
        }
        Err(map_error_response(code, response).await)
    }

    /// POST/PUT/PATCH that needs both a query string and a JSON body.
    pub(crate) async fn send_with_query_and_body<T, Q, B>(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
        query: &Q,
        body: &B,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
        B: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send_with_query_and_body(method, path, query, body);
        }
        let rb = self.request(auth, method, path).await?.query(query).json(body);
        let raw = self.execute_cached(Some(auth), rb, cache, auth_scoped).await?;
        decode_body(&raw)
    }

    /// Like `send` but discards a 2xx body.
    pub(crate) async fn send_no_response<B>(
        &self,
        auth: &AuthMode,
        method: Method,
        path: &str,
        body: Option<&B>,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<(), Error>
    where
        B: Serialize + ?Sized,
    {
        let _ = (cache, auth_scoped);
        if self.mock {
            return crate::x::mock::send_no_response(method, path, body);
        }
        let mut rb = self.request(auth, method, path).await?;
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let req = rb.build().map_err(Error::RequestBuild)?;
        let response = self.client.execute(req).await.map_err(Error::Transport)?;
        let code = response.status();
        if code.is_success() {
            return Ok(());
        }
        Err(map_error_response(code, response).await)
    }

    /// Fetch the raw response body from an arbitrary URL (twimg
    /// media downloads). Always cached, never auth-scoped (media
    /// URLs are persona-independent). No `authorization` header.
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, Error> {
        if self.mock {
            return Err(Error::Other(
                "fetch_url not supported in mock mode".into(),
            ));
        }
        let rb = self.client.get(url);
        // No auth: twimg media is persona-independent and never
        // auth-scoped.
        self.execute_cached(None, rb, /* cache */ true, /* auth_scoped */ false).await
    }

    /// Build the request, then either route through the cache or
    /// fire it directly. Returns raw 2xx response bytes. When
    /// `auth_scoped` is set, the cache key folds in the
    /// authenticated twid so distinct agents (sharing this Client's
    /// cache file) get distinct entries.
    async fn execute_cached(
        &self,
        auth: Option<&AuthMode>,
        rb: reqwest::RequestBuilder,
        cache: bool,
        auth_scoped: bool,
    ) -> Result<Vec<u8>, Error> {
        let req = rb.build().map_err(Error::RequestBuild)?;
        if cache {
            let key = if auth_scoped {
                let auth = auth.expect("auth_scoped requests must pass an AuthMode");
                let twid = self.current_twid(auth).await.map_err(Error::Authorization)?;
                auth_scoped_key_from_request(&twid, &req)
            } else {
                key_from_request(&req)
            };
            self.db
                .cache_get_or_fetch(
                    &key,
                    self.cache_max_size,
                    self.cache_ttl,
                    || async move { run_request_raw(self.client.clone(), req).await },
                )
                .await
        } else {
            run_request_raw(self.client.clone(), req).await
        }
    }
}

fn key_from_request(req: &reqwest::Request) -> [u8; 32] {
    let body = req
        .body()
        .and_then(|b| b.as_bytes())
        .unwrap_or(&[]);
    request_key(req.method(), req.url().as_str(), &[], body)
}

fn auth_scoped_key_from_request(twid: &str, req: &reqwest::Request) -> [u8; 32] {
    let body = req
        .body()
        .and_then(|b| b.as_bytes())
        .unwrap_or(&[]);
    request_key_auth_scoped(twid, req.method(), req.url().as_str(), &[], body)
}

async fn run_request_raw(
    client: ReqwestClient,
    req: reqwest::Request,
) -> Result<Vec<u8>, Error> {
    let response = client.execute(req).await.map_err(Error::Transport)?;
    let code = response.status();
    let bytes = response.bytes().await.map_err(Error::Transport)?;
    if code.is_success() {
        return Ok(bytes.to_vec());
    }
    let text = String::from_utf8_lossy(&bytes);
    Err(map_status_error(code, &text))
}

fn decode_body<T: DeserializeOwned>(raw: &[u8]) -> Result<T, Error> {
    let mut de = serde_json::Deserializer::from_slice(raw);
    serde_path_to_error::deserialize::<_, T>(&mut de).map_err(Error::Deserialize)
}

fn map_status_error(code: StatusCode, text: &str) -> Error {
    if let Ok(problem) = serde_json::from_str::<Problem>(text) {
        return Error::Problem { code, problem };
    }
    let body = serde_json::from_str::<serde_json::Value>(text)
        .unwrap_or_else(|_| serde_json::Value::String(text.to_string()));
    Error::BadStatus { code, body }
}

async fn map_error_response(code: StatusCode, response: reqwest::Response) -> Error {
    match response.text().await {
        Ok(text) => map_status_error(code, &text),
        Err(e) => Error::Other(format!("error body read: {e}")),
    }
}
