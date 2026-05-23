//! Everything the browser writes to stdout.
//!
//! Wire format: one JSON object per line, externally tagged on `"type"`.
//! The browser never prints to stdout or stderr outside this enum —
//! all output, including diagnostics and argument-parse errors, flows
//! through here as JSONL.

use serde::{Deserialize, Serialize};

use crate::response::ResponseOutcome;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// A reply to a previously-received [`crate::request::Request`].
    /// The inner [`ResponseOutcome`] is either an `ok` response or an
    /// `err` with a textual reason.
    Response {
        #[serde(flatten)]
        result: ResponseOutcome,
    },

    /// A diagnostic line — anything the browser used to write to
    /// stderr (parse errors, lifecycle traces, etc.).
    Log { message: String },
}
