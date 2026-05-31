//! X v2 API client.
//!
//! Single constructor [`Client::new`] — **infallible** and
//! **synchronous**. No I/O happens at construction time: the
//! SQLite cache + auth locker open lazily on first use via
//! `tokio::sync::OnceCell`, and the wire bearer is resolved per
//! request by reading the auth files referenced by
//! [`Client::auth_mode`] ("on the fly"). Live changes to the
//! signed-in X-App or persona (browser sign-out / sign-in) are
//! picked up on the next request without rebuilding the Client.
//!
//! Auth file ownership lives here too: [`Client::read_auth`] is
//! a cheap lockless read, [`Client::lock_auth`] acquires the
//! two-tier (in-process tokio mutex + cross-process SQLite
//! `locks` table) lock, and [`Client::write_auth`] consumes the
//! lock and atomically writes the file. The
//! [`super::auth::AuthLock`] returned by `lock_auth` cannot be
//! constructed externally — `lock_auth` is its only producer.
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
use tokio::sync::OnceCell;

use super::Error;
use super::auth::{self, AuthLock, PersonaKey};
use super::cache::{Cache, request_key};
use super::locker::Locker;
use crate::browser::auth_json::{self, PersonaKind, Tokens};
use crate::browser::cookies;
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
    pub(crate) config_base_dir: Arc<PathBuf>,
    pub(crate) auth_mode: AuthMode,
    pub(crate) cache: OnceCell<Arc<Cache>>,
    pub(crate) auth_locker: OnceCell<Arc<Locker>>,
}

impl Client {
    /// Build an X v2 API client. **Infallible** and
    /// **synchronous** — no I/O happens here. The SQLite
    /// cache + auth locker open lazily on first use; the bearer
    /// is resolved per request from the auth files referenced
    /// by `auth_mode`.
    pub fn new(
        client: ReqwestClient,
        mock: bool,
        cache_max_size: u64,
        cache_ttl: Duration,
        config_base_dir: PathBuf,
        auth_mode: AuthMode,
    ) -> Self {
        Self {
            client,
            mock,
            cache_max_size,
            cache_ttl,
            config_base_dir: Arc::new(config_base_dir),
            auth_mode,
            cache: OnceCell::new(),
            auth_locker: OnceCell::new(),
        }
    }

    // ===================================================================
    // Lazy accessors for the SQLite cache + auth locker.
    // ===================================================================

    /// Open the SQLite cache + table on first call; subsequent
    /// calls return the cached `Arc`.
    pub(crate) async fn cache(&self) -> Result<&Arc<Cache>, Error> {
        self.cache
            .get_or_try_init(|| async {
                let c = Cache::open(
                    &self.config_base_dir,
                    self.cache_max_size,
                    self.cache_ttl,
                )
                .await?;
                Ok::<_, Error>(Arc::new(c))
            })
            .await
    }

    /// Auth locker shares the cache's pool. Initialized on first
    /// call; transitively initializes [`cache`].
    pub(crate) async fn auth_locker(&self) -> Result<&Arc<Locker>, Error> {
        self.auth_locker
            .get_or_try_init(|| async {
                let cache = self.cache().await?;
                Ok::<_, Error>(Arc::new(Locker::new(cache.pool().clone())))
            })
            .await
    }

    // ===================================================================
    // Auth-file surface (read / lock / write).
    // ===================================================================

    /// Read `auth.json` for `persona`. No locking, no twid
    /// resolution — just opens the file and parses. Returns
    /// `Ok(None)` if the file doesn't exist.
    pub fn read_auth(&self, persona: &PersonaKey) -> Result<Option<Tokens>, Error> {
        let auth_path = self.auth_path(persona);
        match std::fs::read(&auth_path) {
            Ok(bytes) => {
                let tokens: Tokens = serde_json::from_slice(&bytes).map_err(|e| {
                    Error::Other(format!("auth.json parse {}: {e}", auth_path.display()))
                })?;
                Ok(Some(tokens))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Other(format!(
                "auth.json read {}: {e}",
                auth_path.display(),
            ))),
        }
    }

    /// Acquire the two-tier auth lock for `persona`. Returned
    /// `AuthLock` is consumed by [`Self::write_auth`] (or dropped
    /// for best-effort release without writing). The lock cannot
    /// be constructed externally — `lock_auth` is the only
    /// producer.
    pub async fn lock_auth(&self, persona: &PersonaKey) -> Result<AuthLock, Error> {
        if self.mock {
            return Err(Error::Other(
                "lock_auth not supported in mock mode".into(),
            ));
        }
        let locker = self.auth_locker().await?;
        let key = auth::auth_lock_key(persona);
        let guard = locker.acquire(&key).await?;
        Ok(AuthLock::new(guard, persona.clone()))
    }

    /// Write `new_data` to the persona's auth.json (atomic via
    /// tempfile + rename), then release the lock (real `DELETE`
    /// of the SQLite row, then drop inproc — awaited).
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
        let auth_path = self.auth_path(lock.persona());
        let dir = auth_path
            .parent()
            .ok_or_else(|| Error::Other("auth.json has no parent dir".into()))?;
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::Other(format!("auth.json mkdir: {e}")))?;
        let tmp_path = auth_path.with_extension("json.tmp");
        let mut json = serde_json::to_vec_pretty(new_data)
            .map_err(|e| Error::Other(format!("auth.json encode: {e}")))?;
        json.push(b'\n');
        tokio::fs::write(&tmp_path, &json)
            .await
            .map_err(|e| Error::Other(format!("auth.json tmp write: {e}")))?;
        tokio::fs::rename(&tmp_path, &auth_path)
            .await
            .map_err(|e| Error::Other(format!("auth.json rename: {e}")))?;
        lock.guard.release().await;
        Ok(())
    }

    /// Compute the auth.json path for `persona`.
    fn auth_path(&self, persona: &PersonaKey) -> PathBuf {
        auth_json::path_for(
            self.config_base_dir.as_path(),
            persona.kind,
            &persona.name,
            &persona.persona_twid,
            &persona.x_app_twid,
        )
    }

    // ===================================================================
    // Per-call auth resolution.
    // ===================================================================

    /// Resolve the current Bearer based on [`self.auth_mode`] —
    /// reads `x_app.json` for `XApp`, runs the read/lock/double-
    /// check/refresh dance for `Psyop`/`Agent`. All I/O happens
    /// here on every call.
    async fn current_bearer_token(&self) -> Result<String, Error> {
        match &self.auth_mode {
            AuthMode::XApp => {
                let x_app = super::x_app::config::load(&self.config_base_dir)?;
                x_app.bearer_token.ok_or_else(|| {
                    Error::Other(
                        "x_app.json has no bearer_token — re-run \
                         `psychological-operations x_app setup` and capture it"
                            .into(),
                    )
                })
            }
            AuthMode::Psyop(name) => {
                self.persona_bearer(PersonaKind::Psyop, name).await
            }
            AuthMode::Agent(name) => {
                self.persona_bearer(PersonaKind::Agent, name).await
            }
        }
    }

    /// Resolve twids fresh from cookies, read auth.json, refresh
    /// through the two-tier lock if stale. Shared by `Psyop` /
    /// `Agent` variants.
    async fn persona_bearer(
        &self,
        kind: PersonaKind,
        name: &str,
    ) -> Result<String, Error> {
        let cookie_mode = match kind {
            PersonaKind::Psyop => Mode::PsyopAuthorize { name: name.to_string() },
            PersonaKind::Agent => Mode::AgentAuthorize { name: name.to_string() },
        };
        let persona_twid = cookies::signed_in_x_user_id(
            &self.config_base_dir,
            &cookie_mode,
        )
        .await
        .map_err(|e| Error::Other(format!("persona cookies: {e}")))?
        .ok_or_else(|| {
            Error::Other(format!("no persona signed in for {kind:?} '{name}'"))
        })?;
        let x_app_twid = cookies::signed_in_x_user_id(
            &self.config_base_dir,
            &Mode::XApp,
        )
        .await
        .map_err(|e| Error::Other(format!("x-app cookies: {e}")))?
        .ok_or_else(|| Error::Other("no X-App account signed in".into()))?;
        let persona = PersonaKey {
            kind,
            name: name.to_string(),
            persona_twid,
            x_app_twid,
        };

        // 1. Cheap, lockless read.
        if let Some(t) = self.read_auth(&persona)? {
            if auth_json::is_fresh(&t) {
                return Ok(t.access_token);
            }
        }
        // 2. Stale or missing — acquire the two-tier lock.
        let lock = self.lock_auth(&persona).await?;
        // 3. Re-read after the lock — someone else may have refreshed.
        let stale = match self.read_auth(&persona)? {
            Some(t) if auth_json::is_fresh(&t) => {
                drop(lock);
                return Ok(t.access_token);
            }
            Some(t) => t,
            None => {
                drop(lock);
                return Err(Error::Other(format!(
                    "no auth.json for persona '{name}' — run the OAuth flow first",
                )));
            }
        };
        let refresh_token = stale.refresh_token.as_deref().ok_or_else(|| {
            Error::Other("auth.json has no refresh_token to refresh against".into())
        })?;
        // 4. Refresh via X's OAuth token endpoint.
        let x_app = super::x_app::config::ensure_setup(&self.config_base_dir)?;
        let client_id = x_app.client_id.expect("ensure_setup guarantees client_id");
        let client_secret = x_app
            .client_secret
            .expect("ensure_setup guarantees client_secret");
        let new_tokens = super::oauth::tokens::refresh(
            &client_id,
            &client_secret,
            refresh_token,
        )
        .await?;
        let access = new_tokens.access_token.clone();
        self.write_auth(lock, &new_tokens).await?;
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
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let token = self.current_bearer_token().await?;
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
        method: Method,
        path: &str,
        query: &Q,
        cache: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send_with_query(method, path, query);
        }
        let rb = self.request(method, path).await?.query(query);
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Send `method` to `path` with an optional JSON body.
    pub(crate) async fn send<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        cache: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send(method, path, body);
        }
        let mut rb = self.request(method, path).await?;
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Like `send_with_query` but discards the response body.
    pub(crate) async fn send_with_query_no_response<Q>(
        &self,
        method: Method,
        path: &str,
        query: &Q,
        cache: bool,
    ) -> Result<(), Error>
    where
        Q: Serialize + ?Sized,
    {
        let _ = cache;
        if self.mock {
            return crate::x::mock::send_with_query_no_response(method, path, query);
        }
        let response = self
            .request(method, path)
            .await?
            .query(query)
            .send()
            .await
            .map_err(Error::Transport)?;
        let code = response.status();
        if code.is_success() {
            return Ok(());
        }
        Err(map_error_response(code, response).await)
    }

    /// POST/PUT/PATCH that needs both a query string and a JSON body.
    pub(crate) async fn send_with_query_and_body<T, Q, B>(
        &self,
        method: Method,
        path: &str,
        query: &Q,
        body: &B,
        cache: bool,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
        B: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send_with_query_and_body(method, path, query, body);
        }
        let rb = self.request(method, path).await?.query(query).json(body);
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Like `send` but discards a 2xx body.
    pub(crate) async fn send_no_response<B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        cache: bool,
    ) -> Result<(), Error>
    where
        B: Serialize + ?Sized,
    {
        let _ = cache;
        if self.mock {
            return crate::x::mock::send_no_response(method, path, body);
        }
        let mut rb = self.request(method, path).await?;
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let response = rb.send().await.map_err(Error::Transport)?;
        let code = response.status();
        if code.is_success() {
            return Ok(());
        }
        Err(map_error_response(code, response).await)
    }

    /// Fetch the raw response body from an arbitrary URL (twimg
    /// media downloads). Always cached. No `authorization` header.
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, Error> {
        if self.mock {
            return Err(Error::Other(
                "fetch_url not supported in mock mode".into(),
            ));
        }
        let rb = self.client.get(url);
        self.execute_cached(rb, /* cache */ true).await
    }

    /// Build the request, then either route through the cache or
    /// fire it directly. Returns raw 2xx response bytes.
    async fn execute_cached(
        &self,
        rb: reqwest::RequestBuilder,
        cache: bool,
    ) -> Result<Vec<u8>, Error> {
        let req = rb.build().map_err(Error::RequestBuild)?;
        if cache {
            let cache = self.cache().await?.clone();
            let key = key_from_request(&req);
            let client = self.client.clone();
            cache
                .get_or_fetch(&key, move || async move {
                    run_request_raw(client, req).await
                })
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
