//! Responses the browser sends back to the host process.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//!
//! Examples:
//! ```text
//! {"type":"html","html":"<!DOCTYPE html…"}
//! {"type":"ack"}
//! {"type":"console","entries":[{…},…]}
//! {"type":"eval","result":2}
//! ```

use serde::{Deserialize, Serialize};

use crate::console::ConsoleEntry;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Reply to [`crate::request::Request::Html`].
    Html { html: String },
    /// Generic acknowledgement. Reply to mode-setting requests like
    /// [`crate::request::Request::XApp`].
    Ack,
    /// Reply to [`crate::request::Request::Console`] — the buffer the
    /// overlay accumulated since the last drain.
    Console { entries: Vec<ConsoleEntry> },
    /// Reply to [`crate::request::Request::Eval`] — the
    /// `JSON.stringify`'d (and re-parsed) result the overlay's eval
    /// produced. Runtime errors come back as
    /// [`ResponseOutcome::Err`] instead.
    Eval { result: serde_json::Value },
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
