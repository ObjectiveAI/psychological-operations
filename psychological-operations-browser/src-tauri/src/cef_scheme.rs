//! `psyops://` custom URL scheme — the JS → Rust call path for the
//! CEF content overlay.
//!
//! The overlay calls
//! `fetch("psyops://invoke/<cmd>", { method: "POST", body: <json> })`.
//! CEF routes to [`PsyopsFactory::create`] (registered via
//! `register_scheme_handler_factory` in [`crate::cef::initialize`]),
//! which builds a per-request [`PsyopsHandler`]. The handler:
//!
//!   1. `process_request` extracts the command name from the URL
//!      path and the JSON body from the post data, then spawns a
//!      `tauri::async_runtime::spawn_blocking` task to call into
//!      [`crate::stdio`]'s inner command functions. Returns `1`
//!      (async = false; we set up the state synchronously before
//!      invoking the callback) — actually returns `1` for sync
//!      handled here means CEF assumes a response is ready
//!      immediately. We need async behavior so we return `1` and
//!      use the `callback` to signal completion. Per CEF docs:
//!      `process_request` returns 1 if the request is being
//!      handled, and `callback.cont()` must be invoked once the
//!      response is ready.
//!   2. When the dispatch finishes, the handler stashes the body
//!      bytes + MIME and calls `callback.cont()`.
//!   3. CEF then calls `response_headers`, which reads the
//!      stashed status/MIME/length.
//!   4. CEF calls `read_response` one or more times, copying
//!      buffered bytes into `data_out`. Returns 0 (no more data)
//!      to signal EOF.
//!
//! The Rust → JS direction does NOT go through this scheme — it
//! uses [`crate::cef::execute_overlay_js`] to inject JavaScript
//! into the main frame directly (fire-and-forget).

use std::sync::{Arc, Mutex};

use cef::*;
use psychological_operations_browser_sdk::response::ResponseOutcome;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Wry};

use crate::stdio;

/// Per-request state shared between the async dispatch task and
/// the CEF `read_response` / `response_headers` callbacks.
#[derive(Default)]
struct ResponseState {
    body: Vec<u8>,
    mime: String,
    status: i32,
    /// Number of bytes already copied into CEF via `read_response`.
    position: usize,
    /// Set once dispatch completes; `read_response` returns EOF
    /// when this is true and `position == body.len()`.
    ready: bool,
}

wrap_scheme_handler_factory! {
    pub struct PsyopsFactory {}

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let _ = psychological_operations_browser_sdk::output::Output::Log {
                message: "cef_scheme: factory.create() called".into(),
            }
            .emit();
            Some(PsyopsHandler::new(Arc::new(Mutex::new(ResponseState::default()))))
        }
    }
}

wrap_resource_handler! {
    struct PsyopsHandler {
        state: Arc<Mutex<ResponseState>>,
    }

    impl ResourceHandler {
        fn process_request(
            &self,
            request: Option<&mut Request>,
            callback: Option<&mut Callback>,
        ) -> i32 {
            let Some(request) = request else { return 0 };
            let Some(callback) = callback else { return 0 };

            // CefStringUserfree (a Drop-owned UTF-16) → CefString
            // (UTF-16, refcounted view) → String via Display. The
            // intermediate `CefStringUtf16::from(&userfree)` is the
            // only conversion path the cef-rs crate exposes.
            let url_userfree = request.url();
            let url = CefStringUtf16::from(&url_userfree).to_string();
            let body = extract_post_body(request);
            let _ = psychological_operations_browser_sdk::output::Output::Log {
                message: format!(
                    "cef_scheme: process_request url={url} body_len={} body={:?}",
                    body.len(),
                    std::str::from_utf8(&body).unwrap_or("<non-utf8>")
                ),
            }
            .emit();
            let state = self.state.clone();
            // `Callback` is refcounted; clone bumps the cef refcount so
            // we can hold it across the async task without it dying.
            let callback = callback.clone();

            tauri::async_runtime::spawn(async move {
                let (status, mime, body_out) = match crate::cef::app_handle() {
                    Some(app) => dispatch(app, &url, &body).await,
                    None => (
                        503,
                        "text/plain; charset=utf-8".to_string(),
                        b"cef not initialized".to_vec(),
                    ),
                };
                let _ = psychological_operations_browser_sdk::output::Output::Log {
                    message: format!(
                        "cef_scheme: dispatched url={url} status={status} body_len={} body={:?}",
                        body_out.len(),
                        std::str::from_utf8(&body_out).unwrap_or("<non-utf8>")
                    ),
                }.emit();
                if let Ok(mut s) = state.lock() {
                    s.body = body_out;
                    s.mime = mime;
                    s.status = status;
                    s.ready = true;
                }
                callback.cont();
            });

            1 // handled — we'll fire callback.cont() when ready
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(response) = response else { return };
            let Ok(state) = self.state.lock() else { return };
            response.set_status(state.status);
            let mime = CefString::from(state.mime.as_str());
            response.set_mime_type(Some(&mime));
            // CORS: the overlay runs at https://x.com origin and
            // fetches psyops:// (cross-origin). Even "simple"
            // requests require the response to carry
            // `Access-Control-Allow-Origin` for the renderer to
            // hand the response to JS — otherwise fetch() rejects
            // with a generic "Failed to fetch".
            let allow_origin_name = CefString::from("Access-Control-Allow-Origin");
            let allow_origin_value = CefString::from("*");
            response.set_header_by_name(
                Some(&allow_origin_name),
                Some(&allow_origin_value),
                1, // overwrite
            );
            if let Some(len) = response_length {
                *len = state.body.len() as i64;
            }
        }

        // Modern `read` (used by Chrome runtime). Sync impl —
        // copy bytes immediately, return 1 for "more might be
        // available", 0 for EOF.
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> i32 {
            self.read_inner(data_out, bytes_to_read, bytes_read)
        }

        // Legacy `read_response` (Alloy runtime / older fallback).
        // Same body as `read`.
        fn read_response(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            self.read_inner(data_out, bytes_to_read, bytes_read)
        }
    }
}

impl PsyopsHandler {
    /// Shared body for both `read` (modern, Chrome runtime) and
    /// `read_response` (legacy, Alloy runtime). Copies as many
    /// bytes as fit into `data_out` from the buffered response
    /// body, advances the position pointer, and returns 1 if more
    /// may be available, 0 if EOF.
    fn read_inner(
        &self,
        data_out: *mut u8,
        bytes_to_read: i32,
        bytes_read: Option<&mut i32>,
    ) -> i32 {
        let Some(bytes_read_out) = bytes_read else { return 0 };
        let Ok(mut state) = self.state.lock() else {
            *bytes_read_out = 0;
            return 0;
        };
        let remaining = state.body.len().saturating_sub(state.position);
        if remaining == 0 {
            *bytes_read_out = 0;
            return 0;
        }
        let to_copy = remaining.min(bytes_to_read.max(0) as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(
                state.body.as_ptr().add(state.position),
                data_out,
                to_copy,
            );
        }
        state.position += to_copy;
        *bytes_read_out = to_copy as i32;
        1
    }
}

/// Pull the request body bytes out of CEF's `PostData` → list of
/// `PostDataElement`. Concatenates all byte-typed elements.
///
/// Subtle binding shape: `PostData::elements(Some(&mut vec))`
/// uses the vec's CURRENT length as the buffer size. CEF fills
/// up to that many slots — it does NOT resize for us. So we
/// must pre-size the vec via `element_count()` before passing
/// it in, otherwise CEF writes 0 elements and our body comes
/// back empty.
fn extract_post_body(request: &mut Request) -> Vec<u8> {
    let Some(post_data) = request.post_data() else {
        return Vec::new();
    };
    let count = post_data.element_count();
    let mut elements: Vec<Option<PostDataElement>> = vec![None; count];
    post_data.elements(Some(&mut elements));
    let mut out = Vec::new();
    for elem in elements.into_iter().flatten() {
        let bytes = elem.bytes_count();
        if bytes == 0 {
            continue;
        }
        let prev = out.len();
        out.resize(prev + bytes, 0);
        elem.bytes(bytes, out[prev..].as_mut_ptr());
    }
    out
}

/// Route a single `psyops://invoke/<cmd>` HTTP-style request to
/// the shared dispatcher. Returns `(status, mime, body)`.
async fn dispatch(app: &AppHandle<Wry>, url: &str, body: &[u8]) -> (i32, String, Vec<u8>) {
    // url looks like `psyops://invoke/<cmd>` (no host, no query).
    let cmd = url
        .strip_prefix("psyops://invoke/")
        .or_else(|| url.strip_prefix("psyops://invoke%2F"))
        .unwrap_or("")
        .trim_end_matches('/');
    if cmd.is_empty() {
        return error(404, format!("not found: {url}"));
    }
    match dispatch_inner(app, cmd, body).await {
        Ok(value) => ok_json(&value),
        Err(DispatchError::NotFound(msg)) => error(404, msg),
        Err(DispatchError::BadRequest(msg)) => error(400, msg),
        Err(DispatchError::Internal(msg)) => error(500, msg),
    }
}

/// Shared dispatch helper, called by both the HTTP scheme handler
/// ([`dispatch`]) and the V8 process-message bridge
/// ([`crate::cef`]). Takes the parsed cmd + raw JSON body and
/// returns the result `Value` (or a structured error).
pub async fn dispatch_inner(
    app: &AppHandle<Wry>,
    cmd: &str,
    body: &[u8],
) -> Result<Value, DispatchError> {
    match cmd {
        "frontend_ready" => stdio::frontend_ready_inner(app)
            .map(|()| Value::Null)
            .map_err(DispatchError::Internal),
        "stdio_respond" => {
            let args = parse_body::<StdioRespondArgs>(body).map_err(DispatchError::BadRequest)?;
            stdio::stdio_respond_inner(app, args.result)
                .map(|()| Value::Null)
                .map_err(DispatchError::Internal)
        }
        "current_user_id" => {
            Ok(serde_json::to_value(stdio::current_user_id_inner()).unwrap_or(Value::Null))
        }
        "current_panel" => {
            Ok(serde_json::to_value(stdio::current_panel_inner()).unwrap_or(Value::Null))
        }
        "report_url" => {
            let args = parse_body::<ReportUrlArgs>(body).map_err(DispatchError::BadRequest)?;
            stdio::report_url_inner(app, args.url)
                .map(|()| Value::Null)
                .map_err(DispatchError::Internal)
        }
        "set_production_app_count" => {
            let args = parse_body::<SetCountArgs>(body).map_err(DispatchError::BadRequest)?;
            stdio::set_production_app_count_inner(app, args.count)
                .map(|()| Value::Null)
                .map_err(DispatchError::Internal)
        }
        "process_post_create_html" => {
            let args = parse_body::<ProcessHtmlArgs>(body).map_err(DispatchError::BadRequest)?;
            stdio::process_post_create_html_inner(app, args.html)
                .await
                .map(Value::from)
                .map_err(DispatchError::Internal)
        }
        "process_oauth_popup_html" => {
            let args = parse_body::<ProcessHtmlArgs>(body).map_err(DispatchError::BadRequest)?;
            stdio::process_oauth_popup_html_inner(app, args.html)
                .await
                .map(Value::from)
                .map_err(DispatchError::Internal)
        }
        "process_read_html" => {
            let args = parse_body::<ProcessHtmlArgs>(body).map_err(DispatchError::BadRequest)?;
            let count = crate::psyop_read::process_html(app, args.html);
            Ok(Value::from(count))
        }
        _ => Err(DispatchError::NotFound(format!("unknown command: {cmd}"))),
    }
}

/// Structured error from [`dispatch_inner`]. Mapped to HTTP
/// status codes by [`dispatch`] and to JSON `{status: "err"}`
/// payloads by the V8 bridge.
pub enum DispatchError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl DispatchError {
    /// Flatten to a single string for the V8 bridge's `result`
    /// field. Detail-level matches what the HTTP wrapper would
    /// have returned in the response body.
    pub fn into_message(self) -> String {
        match self {
            DispatchError::NotFound(s) | DispatchError::BadRequest(s) | DispatchError::Internal(s) => s,
        }
    }
}

fn ok_json(v: &Value) -> (i32, String, Vec<u8>) {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"null".to_vec());
    (200, "application/json; charset=utf-8".to_string(), body)
}

fn error(status: i32, msg: impl Into<String>) -> (i32, String, Vec<u8>) {
    let body = msg.into().into_bytes();
    (status, "text/plain; charset=utf-8".to_string(), body)
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, String> {
    // Treat an empty body as `{}` for arg-less commands invoked
    // through this path with no body.
    let bytes: &[u8] = if body.is_empty() { b"{}" } else { body };
    serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON body: {e}"))
}

// ---- per-command arg shapes (mirror the Tauri command signatures) ----

#[derive(Deserialize)]
struct StdioRespondArgs {
    result: ResponseOutcome,
}

#[derive(Deserialize)]
struct ReportUrlArgs {
    url: String,
}

#[derive(Deserialize)]
struct SetCountArgs {
    count: Option<u32>,
}

#[derive(Deserialize)]
struct ProcessHtmlArgs {
    html: String,
}
