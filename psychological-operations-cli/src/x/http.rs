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
}

impl Http {
    /// Construct a new client. `base_url` defaults to
    /// `https://api.x.com/2` when `None`.
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
        }
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
    pub async fn send_get<T, Q>(&self, path: &str, query: &Q) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
    {
        let rb = self.request(Method::GET, path).query(query);
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
        let mut rb = self.request(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
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
