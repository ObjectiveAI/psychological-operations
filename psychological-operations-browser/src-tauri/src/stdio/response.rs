//! Responses the browser writes back to stdout.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//! Each variant carries either a success payload or an error string.
//!
//! Examples:
//! ```text
//! {"type":"html","html":"<!DOCTYPE html…"}
//! {"type":"html","error":"no active web contents"}
//! ```

use serde::{Deserialize, Serialize};

// `Deserialize` is needed so the frontend can hand the response back
// to Rust via the `stdio_respond` Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Reply to [`crate::stdio::request::Request::Html`].
    Html(HtmlPayload),
}

/// Outcome of an [`Response::Html`] response — either the serialized
/// outer HTML or an error string explaining why it wasn't available.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HtmlPayload {
    Ok {
        html: String,
    },
    Err {
        error: String,
    },
}
