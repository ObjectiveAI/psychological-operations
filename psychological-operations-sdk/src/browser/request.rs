//! Requests the host process sends to the browser.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Mode is set at the browser's CLI flags and locked for the
//! lifetime of the process — there's no runtime mode-switch
//! request. To change mode, kill the browser and relaunch it.
//!
//! Examples:
//! ```text
//! {"type":"html"}
//! {"type":"console"}
//! {"type":"eval","code":"document.title"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask for the active page's serialized outer HTML.
    Html,
    /// Drain the overlay's buffered console-entry buffer. Returns
    /// every `console.log/warn/error/info/debug` call and every
    /// uncaught exception captured since the last `Console` drain.
    Console,
    /// Evaluate arbitrary JS in the overlay's window context. The
    /// expression's value (after `Promise` resolution and
    /// JSON-serialization) comes back as
    /// [`crate::browser::response::Response::Eval`]; a runtime throw becomes a
    /// [`crate::browser::response::ResponseOutcome::Err`].
    Eval { code: String },
}
