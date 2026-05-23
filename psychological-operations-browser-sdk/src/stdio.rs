//! JSON-Lines stdio protocol wire types + event-name constants.
//!
//! Flow:
//! 1. The host process writes one [`Request`] per line to the
//!    browser's stdin.
//! 2. The browser parses each line and emits it to the frontend on
//!    the [`EVENT_REQUEST`] Tauri event.
//! 3. The frontend handles the request and posts a [`Response`] back
//!    via the `stdio_respond` Tauri command.
//! 4. The browser writes the response as one JSON line to stdout.
//!
//! Both wire types are externally tagged on a `"type"` field
//! (e.g. `{"type":"html"}` request → `{"type":"html","html":"…"}`
//! response).

pub mod request;
pub mod response;

pub use request::Request;
pub use response::{HtmlPayload, Response};

/// Tauri event channel the browser emits stdio requests on.
/// Follows the `psyops:<topic>:<event>` convention.
pub const EVENT_REQUEST: &str = "psyops:stdio:request";
