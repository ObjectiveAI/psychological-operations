//! Requests the host process sends to the browser.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html"}
//! {"type":"x_app"}
//! {"type":"console"}
//! {"type":"eval","code":"document.title"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask for the active page's serialized outer HTML.
    Html,
    /// Place the browser in X-App (master root) mode. Triggers
    /// navigation to `https://console.x.ai/` and an Ack.
    XApp,
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
