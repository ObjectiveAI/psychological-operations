//! Requests the host process can send to the browser via stdin.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html"}
//! ```

use serde::{Deserialize, Serialize};

// `Serialize` is needed so the request can be forwarded to the
// frontend over Tauri's event system.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask for the active page's serialized outer HTML.
    /// Response: [`crate::stdio::response::Response::Html`].
    Html,
}
