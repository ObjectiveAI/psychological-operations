//! Requests the host process sends to the browser.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html"}
//! {"type":"x_app"}
//! {"type":"psyop_read","name":"my-campaign"}
//! {"type":"psyop_authorize","name":"my-campaign"}
//! {"type":"console"}
//! {"type":"eval","code":"document.title"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask for the active page's serialized outer HTML.
    Html,
    /// Switch the browser to X-App mode. If the browser is already
    /// in X-App mode, no-op + Ack. Otherwise: tear down the current
    /// CEF browser, open a new one with the X-App `RequestContext`
    /// pointed at `https://console.x.com/`. Stdin reading blocks
    /// until the new overlay reports ready.
    XApp,
    /// Switch the browser to a Psyop **read** session named
    /// `<name>`. Same teardown / reopen flow as [`Self::XApp`],
    /// but with a per-psyop `RequestContext` (isolated cookies
    /// / storage) pointed at `https://x.com/`. The overlay
    /// streams page HTML to Rust as the persona browses; Rust
    /// dedups and emits new tweet IDs to stdout.
    PsyopRead { name: String },
    /// Switch the browser to a Psyop **authorize** session
    /// named `<name>`. Same teardown / reopen flow but with
    /// Rust additionally driving the persona through X's
    /// OAuth 2.0 PKCE consent screen on sign-in and writing
    /// the resulting tokens to
    /// `<psyop-data-dir>/handles/<persona-twid>/auth.json`.
    PsyopAuthorize { name: String },
    /// Drain the overlay's buffered console-entry buffer. Returns
    /// every `console.log/warn/error/info/debug` call and every
    /// uncaught exception captured since the last `Console` drain.
    Console,
    /// Evaluate arbitrary JS in the overlay's window context. The
    /// expression's value (after `Promise` resolution and
    /// JSON-serialization) comes back as
    /// [`crate::response::Response::Eval`]; a runtime throw becomes a
    /// [`crate::response::ResponseOutcome::Err`].
    Eval { code: String },
}
