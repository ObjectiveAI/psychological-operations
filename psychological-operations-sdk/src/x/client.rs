//! X v2 API client.
//!
//! Owns the reqwest client, base URL, response cache, and the
//! two-tier auth-file lock. The codegen'd per-endpoint helpers in
//! `crate::x::*::{get,post,put,delete}::http` route through the
//! `pub(crate)` `send_*` family on this type — external callers
//! drive everything via the codegen helpers, never the generic
//! methods.
//!
//! Auth file (`auth.json`) ownership lives here too:
//! [`Client::read_auth`] is a cheap lockless read, [`Client::lock_auth`]
//! acquires the two-tier lock, [`Client::write_auth`] consumes the
//! lock and atomically writes the file. The [`super::auth::AuthLock`]
//! returned by `lock_auth` cannot be constructed externally — only
//! `lock_auth` produces one.
//!
//! For [`AuthMode::Persona`] clients, every request resolves the
//! bearer dynamically via `current_bearer_token`:
//!
//!   1. Cheap read of `auth.json` (no lock).
//!   2. If `expires_at` is more than 30 s away, use the token as-is.
//!   3. Otherwise acquire the auth lock, re-read (someone else
//!      may have refreshed while we waited), and if STILL stale,
//!      refresh via X's OAuth token endpoint and write back via
//!      `write_auth` (which releases the lock).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::{Client as ReqwestClient, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Error;
use super::auth::{self, AuthLock, PersonaKey};
use super::cache::{self, Cache, request_key};
use super::locker::Locker;
use crate::browser::auth_json::{self, PersonaKind, Tokens};
use crate::browser::cookies;
use crate::browser::mode::Mode;
use crate::x::types::Problem;

/// Default base URL for the X v2 API.
pub const DEFAULT_BASE_URL: &str = "https://api.x.com/2";

/// X v2 API client. See module docs.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) client: ReqwestClient,
    pub(crate) base_url: String,
    pub(crate) auth: AuthMode,
    /// When true, every `send*` short-circuits to
    /// `crate::x::mock::*` instead of hitting the real X API.
    pub(crate) mock: bool,
    pub(crate) max_size: u64,
    pub(crate) cache: Option<Arc<Cache>>,
    /// Locker for the persona auth lock. `Some` whenever
    /// `cache` is `Some` (they share the SQLite pool); `None`
    /// in mock / no-cache mode.
    pub(crate) auth_locker: Option<Arc<Locker>>,
    /// Process-wide config_base_dir the constructors were given.
    /// Used to compute auth.json paths. `None` in mock mode.
    pub(crate) config_base_dir: Option<Arc<PathBuf>>,
}

/// How this `Client` produces a Bearer for outgoing requests.
#[derive(Debug, Clone)]
pub(crate) enum AuthMode {
    /// Static `x_app.bearer_token`. No refresh; the token is the
    /// permanent app-only key.
    AppOnly { token: Arc<String> },
    /// Per-persona OAuth tokens stored in
    /// `<config>/.../handles/<persona_twid>/<x_app_twid>/auth.json`.
    /// Refreshed on demand when expiring within
    /// [`auth_json::FRESHNESS_BUFFER`].
    Persona {
        persona: PersonaKey,
        client_id: Arc<String>,
        client_secret: Arc<String>,
    },
}

impl Client {
    /// Low-level constructor — `pub(crate)` so external callers go
    /// through [`Client::app_only`] or [`Client::for_psyop`].
    pub(crate) fn new(
        client: ReqwestClient,
        base_url: Option<impl Into<String>>,
        auth: AuthMode,
        max_size: u64,
        cache: Option<Arc<Cache>>,
        auth_locker: Option<Arc<Locker>>,
        config_base_dir: Option<Arc<PathBuf>>,
    ) -> Self {
        Self {
            client,
            base_url: base_url
                .map(Into::into)
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            auth,
            mock: false,
            max_size,
            cache,
            auth_locker,
            config_base_dir,
        }
    }

    /// Mock factory — produces a Client that short-circuits every
    /// `send*` to the mock layer. No auth, no cache, no locker.
    fn new_mock(client: ReqwestClient) -> Self {
        Self {
            client,
            base_url: DEFAULT_BASE_URL.to_string(),
            auth: AuthMode::AppOnly { token: Arc::new(String::new()) },
            mock: true,
            max_size: 0,
            cache: None,
            auth_locker: None,
            config_base_dir: None,
        }
    }

    /// Construct a Client for app-only use. Reads
    /// `x_app.json::bearer_token` (durable, not OAuth-refreshable).
    /// When `mock` is true, skips the read and mocks every send.
    pub async fn app_only(
        client: ReqwestClient,
        mock: bool,
        config_base_dir: &Path,
        max_size: u64,
    ) -> Result<Self, Error> {
        if mock {
            return Ok(Self::new_mock(client));
        }
        let x_app = super::x_app::config::load(config_base_dir)?;
        let bearer = x_app.bearer_token.ok_or_else(|| {
            Error::Other(
                "x_app.json has no bearer_token — re-run \
                 `psychological-operations x_app setup` and capture it".into(),
            )
        })?;
        let cache = cache::open_optional(config_base_dir, max_size).await?;
        let auth_locker = cache
            .as_ref()
            .map(|c| Arc::new(Locker::new(c.pool().clone())));
        Ok(Self::new(
            client,
            None::<&str>,
            AuthMode::AppOnly { token: Arc::new(bearer) },
            max_size,
            cache,
            auth_locker,
            Some(Arc::new(config_base_dir.to_path_buf())),
        ))
    }

    /// Construct a Client authorized as the per-psyop X user.
    /// Resolves both twids (persona + X-App) from the CEF cookie
    /// jars at construction time, then builds an `AuthMode::Persona`.
    /// The actual bearer is resolved lazily on every API call via
    /// `current_bearer_token` — auth.json is read fresh each time,
    /// and refreshed when expiring within
    /// [`auth_json::FRESHNESS_BUFFER`].
    pub async fn for_psyop(
        client: ReqwestClient,
        psyop_name: &str,
        mock: bool,
        config_base_dir: &Path,
        max_size: u64,
    ) -> Result<Self, Error> {
        if mock {
            return Ok(Self::new_mock(client));
        }
        let x_app = super::x_app::config::ensure_setup(config_base_dir)?;
        let client_id = x_app.client_id.expect("ensure_setup guarantees client_id");
        let client_secret = x_app.client_secret.expect("ensure_setup guarantees client_secret");

        let persona_twid = cookies::signed_in_x_user_id(
            config_base_dir,
            &Mode::PsyopAuthorize { name: psyop_name.to_string() },
        )
        .await
        .map_err(|e| Error::Other(format!("persona cookies: {e}")))?
        .ok_or_else(|| Error::Other(format!(
            "no persona signed in for psyop '{psyop_name}'",
        )))?;
        let x_app_twid = cookies::signed_in_x_user_id(config_base_dir, &Mode::XApp)
            .await
            .map_err(|e| Error::Other(format!("x-app cookies: {e}")))?
            .ok_or_else(|| Error::Other("no X-App account signed in".into()))?;

        let persona = PersonaKey {
            kind: PersonaKind::Psyop,
            name: psyop_name.to_string(),
            persona_twid,
            x_app_twid,
        };

        let cache = cache::open_optional(config_base_dir, max_size).await?;
        let auth_locker = cache
            .as_ref()
            .map(|c| Arc::new(Locker::new(c.pool().clone())));
        Ok(Self::new(
            client,
            None::<&str>,
            AuthMode::Persona {
                persona,
                client_id: Arc::new(client_id),
                client_secret: Arc::new(client_secret),
            },
            max_size,
            cache,
            auth_locker,
            Some(Arc::new(config_base_dir.to_path_buf())),
        ))
    }

    // ===================================================================
    // Auth-file surface (read / lock / write).
    // ===================================================================

    /// Read `auth.json` for `persona`. No locking, no twid
    /// resolution — just opens the file and parses. Returns
    /// `Ok(None)` if the file doesn't exist.
    pub fn read_auth(&self, persona: &PersonaKey) -> Result<Option<Tokens>, Error> {
        let auth_path = self.auth_path(persona)?;
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
    /// be constructed externally — `lock_auth` is the only producer.
    pub async fn lock_auth(&self, persona: &PersonaKey) -> Result<AuthLock, Error> {
        let locker = self.auth_locker.as_ref().ok_or_else(|| {
            Error::Other(
                "no auth_locker on this Client — mock or no-cache mode \
                 doesn't support lock_auth".into(),
            )
        })?;
        let key = auth::auth_lock_key(persona);
        let guard = locker.acquire(&key).await?;
        Ok(AuthLock::new(guard, persona.clone()))
    }

    /// Write `new_data` to the persona's auth.json (atomic via
    /// tempfile + rename), then release the lock (real `DELETE`
    /// of the SQLite row, then drop inproc — awaited).
    ///
    /// The persona is taken from `lock.persona()`; you can only
    /// write to the persona you locked.
    pub async fn write_auth(
        &self,
        lock: AuthLock,
        new_data: &Tokens,
    ) -> Result<(), Error> {
        let auth_path = self.auth_path(lock.persona())?;
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

    /// Compute the auth.json path for `persona`. Requires the
    /// Client was constructed with a `config_base_dir` (i.e. not
    /// mock mode).
    fn auth_path(&self, persona: &PersonaKey) -> Result<PathBuf, Error> {
        let base = self.config_base_dir.as_ref().ok_or_else(|| {
            Error::Other(
                "no config_base_dir on this Client — mock mode doesn't \
                 support auth file methods".into(),
            )
        })?;
        Ok(auth_json::path_for(
            base.as_path(),
            persona.kind,
            &persona.name,
            &persona.persona_twid,
            &persona.x_app_twid,
        ))
    }

    /// Run the read-or-refresh dance and return the current
    /// access token to attach as Bearer.
    async fn current_bearer_token(&self) -> Result<String, Error> {
        match &self.auth {
            AuthMode::AppOnly { token } => Ok((**token).clone()),
            AuthMode::Persona { persona, client_id, client_secret } => {
                // 1. Cheap, lockless read.
                if let Some(t) = self.read_auth(persona)? {
                    if auth_json::is_fresh(&t) {
                        return Ok(t.access_token);
                    }
                }
                // 2. Stale (or missing) — acquire the two-tier lock.
                let lock = self.lock_auth(persona).await?;
                // 3. Re-read after the lock — someone else may have refreshed.
                let stale = match self.read_auth(persona)? {
                    Some(t) if auth_json::is_fresh(&t) => {
                        drop(lock);
                        return Ok(t.access_token);
                    }
                    Some(t) => t,
                    None => {
                        drop(lock);
                        return Err(Error::Other(format!(
                            "no auth.json for persona '{}' — run the OAuth flow first",
                            persona.name,
                        )));
                    }
                };
                let refresh_token = stale.refresh_token.as_deref().ok_or_else(|| {
                    Error::Other("auth.json has no refresh_token to refresh against".into())
                })?;
                // 4. Refresh via X's OAuth token endpoint.
                let new_tokens =
                    super::oauth::tokens::refresh(client_id, client_secret, refresh_token).await?;
                let access = new_tokens.access_token.clone();
                self.write_auth(lock, &new_tokens).await?;
                Ok(access)
            }
        }
    }

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
            self.base_url.trim_end_matches('/'),
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
        match (cache, self.cache.as_ref()) {
            (true, Some(c)) => {
                let key = key_from_request(&req);
                let client = self.client.clone();
                let cache = c.clone();
                cache
                    .get_or_fetch(&key, move || async move {
                        run_request_raw(client, req).await
                    })
                    .await
            }
            _ => run_request_raw(self.client.clone(), req).await,
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
