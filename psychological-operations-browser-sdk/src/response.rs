//! Responses the browser sends back to the host process.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html","html":"<!DOCTYPE html…"}
//! {"type":"ack"}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Reply to [`crate::request::Request::Html`].
    Html { html: String },
    /// Generic acknowledgement. Reply to [`crate::request::Request::XApp`]
    /// (and future mode-setting requests) confirming the browser
    /// accepted the request and is now in the requested mode.
    Ack,
}

/// Outcome of handling a request — either the corresponding
/// [`Response`] (ok) or an error string explaining why it couldn't
/// be produced. Carried inside [`crate::output::Output::Response`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseOutcome {
    Ok { response: Response },
    Err { error: String },
}
