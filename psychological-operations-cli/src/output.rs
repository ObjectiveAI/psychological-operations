//! Wire-format emission sink. **The only place in the CLI that
//! writes to stdout** — every byte the host sees passes through
//! [`OutputResult::emit`].
//!
//! Construct an [`OutputResult`] inline at the call site (no
//! delegation helpers), then call `.emit()`. The serialized shape
//! matches `objectiveai_sdk::cli::plugins::Output` so the host
//! parses each line via that enum's untagged discrimination
//! (`type:"error"` → Error; `type:"command"` → Command; everything
//! else → Notification catch-all).
//!
//! Variants:
//!
//! - [`OutputResult::Output`] — terminal success result of a
//!   command, carrying our SDK's [`psychological_operations_sdk::cli::Output`]
//!   payload. Host re-emits as a Notification carrying the inner
//!   Output enum verbatim.
//! - [`OutputResult::Error`] — failure / warning. Host re-emits
//!   as an Error frame.
//! - [`OutputResult::Notification`] — mid-command progress event
//!   (e.g. `BrowseStarting`, `StageBegin`). Carries any JSON value;
//!   host re-emits as a Notification.

use objectiveai_sdk::cli::{Error as ObjError, ErrorType, Level};
use psychological_operations_sdk::cli::Output;
use serde::Serialize;

use crate::events::Event;

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OutputResult {
    /// Terminal `Ok` result of a command.
    Output(Output),
    /// Failure or non-fatal warning.
    Error(ObjError),
    /// Mid-command progress notification carrying any JSON value.
    Notification(serde_json::Value),
}

impl OutputResult {
    /// Serialize and write one JSONL line to stdout. **Only**
    /// `println!` site in the crate.
    pub fn emit(&self) {
        let line = serde_json::to_string(self).expect("OutputResult serializes");
        println!("{line}");
    }

    /// Build an Error variant from level + fatal + message JSON.
    pub fn error(level: Level, fatal: bool, message: serde_json::Value) -> Self {
        OutputResult::Error(ObjError {
            r#type: ErrorType::Error,
            level: Some(level),
            fatal: Some(fatal),
            message,
        })
    }
}

impl From<Event> for OutputResult {
    /// Route an [`Event`] through the right variant per
    /// [`Event::error_level`]: failure-flavored variants land as
    /// [`OutputResult::Error`] with `fatal = false`; everything
    /// else as [`OutputResult::Notification`].
    fn from(event: Event) -> Self {
        let value = serde_json::to_value(&event).expect("Event serializes");
        match event.error_level() {
            Some(level) => OutputResult::error(level, /* fatal */ false, value),
            None => OutputResult::Notification(value),
        }
    }
}
