//! Responses the browser sends back to the host process.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html","html":"<!DOCTYPE html…"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Reply to [`crate::request::Request::Html`].
    Html { html: String },
}
