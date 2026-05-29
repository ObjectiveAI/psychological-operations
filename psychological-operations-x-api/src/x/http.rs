//! HTTP client for the X v2 API.

use std::sync::Arc;

use reqwest::{Client, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Error;
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
    ) -> Self {
        Self {
            client,
            base_url: base_url
                .map(Into::into)
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            bearer_token: bearer_token.map(|t| Arc::new(t.into())),
            mock: false,
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
        cfg: &crate::run::Config,
    ) -> Result<Self, crate::error::Error> {
        if mock {
            return Ok(Self::new_mock(client));
        }
        let x_app = crate::x_app::config::load(cfg)?;
        let bearer = x_app.bearer_token.ok_or_else(|| {
            crate::error::Error::Other(
                "x_app.json has no bearer_token — re-run \
                 `psychological-operations x_app setup` and capture it".into(),
            )
        })?;
        Ok(Self::new(client, None::<&str>, Some(bearer)))
    }

    /// Construct an Http authorized as the per-psyop X user. Reads
    /// `tokens/<psyop>.json`; refreshes silently via
    /// `oauth::tokens::load_fresh` if the access token is expired
    /// or expiring within 5 minutes (uses `x_app.json`'s
    /// `client_id` / `client_secret` for the refresh, persists the
    /// rotated tokens back).
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
        cfg: &crate::run::Config,
    ) -> Result<Self, crate::error::Error> {
        if mock {
            return Ok(Self::new_mock(client));
        }
        let x_app = crate::x_app::config::ensure_setup(cfg)?;
        let client_id = x_app.client_id
            .expect("ensure_setup guarantees client_id");
        let client_secret = x_app.client_secret
            .expect("ensure_setup guarantees client_secret");
        let tokens = crate::oauth::tokens::load_fresh(
            psyop_name, &client_id, &client_secret, cfg,
        ).await?;
        Ok(Self::new(client, None::<&str>, Some(tokens.access_token)))
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
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
    {
        if self.mock {
            return crate::x::mock::send_with_query(method, path, query);
        }
        let rb = self.request(method, path).query(query);
        Self::execute_unary(rb).await
    }

    /// Send `method` to `path` with an optional JSON body. Use for
    /// POST/PUT/PATCH/DELETE.
    pub async fn send<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
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
        Self::execute_unary(rb).await
    }

    /// Like `send_with_query` but discards the response body — useful
    /// for endpoints that return 204 No Content or non-JSON content.
    pub async fn send_with_query_no_response<Q>(
        &self,
        method: Method,
        path: &str,
        query: &Q,
    ) -> Result<(), Error>
    where
        Q: Serialize + ?Sized,
    {
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
        Self::execute_unary(rb).await
    }

    /// Like `send` but discards a 2xx body — useful for endpoints
    /// that return 204 No Content.
    pub async fn send_no_response<B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), Error>
    where
        B: Serialize + ?Sized,
    {
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

    /// Send a built `RequestBuilder`, expecting a 2xx JSON body that
    /// deserializes into `T`. On non-2xx, prefers `Error::Problem`
    /// when the body parses as `Problem`, else falls back to
    /// `Error::BadStatus`.
    async fn execute_unary<T>(rb: reqwest::RequestBuilder) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let response = rb.send().await.map_err(Error::Transport)?;
        let code = response.status();
        let text = response.text().await.map_err(Error::Transport)?;

        if code.is_success() {
            let mut de = serde_json::Deserializer::from_str(&text);
            return serde_path_to_error::deserialize::<_, T>(&mut de)
                .map_err(Error::Deserialize);
        }
        Err(map_status_error(code, &text))
    }
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
