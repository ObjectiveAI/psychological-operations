//! HTTP client for the X v2 API.

use std::path::Path;
use std::sync::Arc;

use crate::browser::auth_json::{self, AuthJsonError};
use reqwest::{Client, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Error;
use crate::x::cache::{Cache, request_key};
use crate::x::types::Problem;

/// Default base URL for the X v2 API.
pub const DEFAULT_BASE_URL: &str = "https://api.x.com/2";

/// HTTP client for the X v2 API.
///
/// Holds a reqwest client, base URL, and an optional Bearer token.
/// All endpoint helpers in `crate::x::*::{get,post,put,delete}` are
/// expected to call into the generic `send_*` methods on this type.
#[derive(Debug, Clone)]
pub struct Http {
    pub client: Client,
    pub base_url: String,
    pub bearer_token: Option<Arc<String>>,
    /// When true, every `send*` short-circuits to
    /// `crate::x::mock::*` instead of hitting the real X API.
    pub mock: bool,
    /// SQLite response-cache size budget, in bytes. Threaded into
    /// [`Cache::open`] when the cache is opened; honored on every
    /// `store`.
    pub max_size: u64,
    /// Optional response cache. `None` when no cache file was
    /// supplied (e.g. `Http::new` callers that don't have a
    /// config_base_dir). When `Some`, `send_*(cache=true)` routes
    /// through it.
    pub cache: Option<Arc<Cache>>,
}

impl Http {
    /// Construct a new client. `base_url` defaults to
    /// `https://api.x.com/2` when `None`. Low-level — most callers
    /// should use `app_only` or `for_psyop` so auth is resolved
    /// from disk automatically.
    pub fn new(
        client: Client,
        base_url: Option<impl Into<String>>,
        bearer_token: Option<impl Into<String>>,
        max_size: u64,
        cache: Option<Arc<Cache>>,
    ) -> Self {
        Self {
            client,
            base_url: base_url
                .map(Into::into)
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            bearer_token: bearer_token.map(|t| Arc::new(t.into())),
            mock: false,
            max_size,
            cache,
        }
    }

    /// Helper for the mock factories — produces an Http that holds
    /// no token and short-circuits every `send*` to the mock layer.
    fn new_mock(client: Client) -> Self {
        Self {
            client,
            base_url: DEFAULT_BASE_URL.to_string(),
            bearer_token: None,
            mock: true,
            max_size: 0,
            cache: None,
        }
    }

    /// Construct an Http for app-only use. Reads
    /// `x_app.json::bearer_token`. Use this for read-only endpoints
    /// (search, tweet lookup) — anything that doesn't need to act
    /// as a specific user.
    ///
    /// When `mock` is true, skips the `x_app.json` read entirely and
    /// returns an Http that mocks every send. Caller derives `mock`
    /// from the psyop's `mock` field (`PsyOp::mock_enabled`).
    pub async fn app_only(
        client: Client,
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
        let cache = open_cache(config_base_dir, max_size)?;
        Ok(Self::new(client, None::<&str>, Some(bearer), max_size, cache))
    }

    /// Construct an Http authorized as the per-psyop X user. The
    /// access token comes through
    /// [`psychological_operations_sdk::browser::auth_json::get_or_refresh`],
    /// which takes a shared filesystem lock to read `auth.json` and
    /// — if the access token expires within
    /// [`auth_json::FRESHNESS_BUFFER`] (currently 30 s) — escalates
    /// to an exclusive lock, re-reads (in case a concurrent process
    /// refreshed in the gap), POSTs to X's token endpoint via this
    /// crate's [`super::oauth::tokens::refresh`], and atomically
    /// writes the rotated tokens back. So one cross-process advisory
    /// lock guards both the read and the refresh-and-write cycle;
    /// no two processes can ever mint a refresh request against the
    /// same stored `refresh_token`.
    ///
    /// Use for write endpoints (likes, retweets) and any read
    /// endpoint that needs user-context scope.
    ///
    /// When `mock` is true, skips OAuth-token loading entirely and
    /// returns an Http that mocks every send. Caller derives `mock`
    /// from the psyop's `mock` field (`PsyOp::mock_enabled`).
    pub async fn for_psyop(
        client: Client,
        psyop_name: &str,
        mock: bool,
        config_base_dir: &Path,
        max_size: u64,
    ) -> Result<Self, Error> {
        if mock {
            return Ok(Self::new_mock(client));
        }
        let x_app = super::x_app::config::ensure_setup(config_base_dir)?;
        let client_id = x_app.client_id
            .expect("ensure_setup guarantees client_id");
        let client_secret = x_app.client_secret
            .expect("ensure_setup guarantees client_secret");
        let tokens = auth_json::get_or_refresh(
            config_base_dir,
            auth_json::PersonaKind::Psyop,
            psyop_name,
            move |stale| async move {
                let rt = stale.refresh_token.as_deref().ok_or_else(|| {
                    AuthJsonError::Io(std::io::Error::other(
                        "auth.json has no refresh_token — re-run \
                         `psychological-operations psyops oauth <name>`",
                    ))
                })?;
                super::oauth::tokens::refresh(&client_id, &client_secret, rt)
                    .await
                    .map_err(|e| AuthJsonError::Io(std::io::Error::other(
                        format!("refresh: {e}"),
                    )))
            },
        )
        .await
        .map_err(|e| Error::Other(format!("auth_json: {e}")))?;
        let cache = open_cache(config_base_dir, max_size)?;
        Ok(Self::new(client, None::<&str>, Some(tokens.access_token), max_size, cache))
    }

    /// Build a `RequestBuilder` for `path` with auth attached. `path`
    /// is appended to `base_url` after stripping any leading `/` and
    /// trailing `/` on the base. Use this when you need to attach
    /// custom headers or multipart bodies that the generic helpers
    /// don't cover.
    pub fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/'),
        );
        let mut rb = self.client.request(method, &url);
        if let Some(token) = &self.bearer_token {
            let bare = token.strip_prefix("Bearer ").unwrap_or(token.as_str());
            rb = rb.header("authorization", format!("Bearer {bare}"));
        }
        rb
    }

    /// GET `path` with `query` URL-encoded. `Q` is the endpoint's
    /// `Request` struct; serde attributes (`csv_vec_opt`, `rename`,
    /// `skip_serializing_if`) are honored by reqwest's `.query()`.
    pub async fn send_with_query<T, Q>(
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
        let rb = self.request(method, path).query(query);
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Send `method` to `path` with an optional JSON body. Use for
    /// POST/PUT/PATCH/DELETE.
    pub async fn send<T, B>(
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
        let mut rb = self.request(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Like `send_with_query` but discards the response body — useful
    /// for endpoints that return 204 No Content or non-JSON content.
    /// `cache` is accepted for signature uniformity but ignored —
    /// there's no body to cache.
    pub async fn send_with_query_no_response<Q>(
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
    /// Used by the rare endpoint with non-path query params alongside a
    /// body (e.g. `POST /2/tweets/search/stream/rules`).
    pub async fn send_with_query_and_body<T, Q, B>(
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
        let rb = self.request(method, path).query(query).json(body);
        let raw = self.execute_cached(rb, cache).await?;
        decode_body(&raw)
    }

    /// Like `send` but discards a 2xx body — useful for endpoints
    /// that return 204 No Content. `cache` is accepted for signature
    /// uniformity but ignored — there's no body to cache.
    pub async fn send_no_response<B>(
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
        let mut rb = self.request(method, path);
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

    /// Build the request, then either route through the cache (on
    /// `cache && self.cache.is_some()`) or fire it directly. Returns
    /// the raw 2xx response bytes for downstream deserialization.
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

/// `SHA-256(method ‖ url ‖ body)` — the url already contains the
/// reqwest-encoded query string, and the body is bytes from
/// `.json(...)` or `.body(...)`. Streaming bodies (none in this
/// crate) would hash to empty.
fn key_from_request(req: &reqwest::Request) -> [u8; 32] {
    let body = req
        .body()
        .and_then(|b| b.as_bytes())
        .unwrap_or(&[]);
    request_key(req.method(), req.url().as_str(), &[], body)
}

async fn run_request_raw(
    client: Client,
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

/// Open the SQLite cache for a given config root, but only when
/// `max_size > 0`. `app_only` / `for_psyop` plumbing.
fn open_cache(
    config_base_dir: &Path,
    max_size: u64,
) -> Result<Option<Arc<Cache>>, Error> {
    if max_size == 0 {
        return Ok(None);
    }
    Ok(Some(Arc::new(Cache::open(config_base_dir, max_size)?)))
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
        Err(_) => Error::BadStatus {
            code,
            body: serde_json::Value::Null,
        },
    }
}
