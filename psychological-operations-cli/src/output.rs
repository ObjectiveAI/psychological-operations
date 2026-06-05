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

// ─── Terminal emission helpers ──────────────────────────────────
//
// Every leaf function callable from a `Commands::handle` arm is
// `-> bool` and uses these three to convert its terminal outcome
// into one final wire-line + the exit-code bool. The handler
// chain just propagates the bool up to `main.rs`.

/// Emit the terminal-success `{"value": ...}` notification for a
/// command's [`Output`]. Output::Empty produces nothing on the
/// wire; everything else stringifies via [`Output`]'s `Display`,
/// is parsed back as JSON if possible (so `Output::ConfigGet`'s
/// JSON-string body unwraps into a real JSON value), and lands in
/// a `Notification` carrying `{"value": <parsed-or-string>}`.
pub fn emit_output(output: Output) {
    let s = output.to_string();
    if s.is_empty() { return; }
    let value: serde_json::Value = serde_json::from_str(&s)
        .unwrap_or_else(|_| serde_json::Value::String(s));
    OutputResult::Notification(serde_json::json!({ "value": value })).emit();
}

/// Emit the fatal-error wire line that previously came out of
/// `main.rs`'s `Err` arm: `Level::Error`, `fatal: true`, message
/// as a JSON string.
pub fn emit_fatal(error: impl std::fmt::Display) {
    OutputResult::error(
        Level::Error,
        /* fatal */ true,
        serde_json::Value::String(error.to_string()),
    )
    .emit();
}

/// Convert a leaf's internal `Result<Output, E>` into the `bool`
/// the handler arm wants: emit terminal-success on `Ok`, fatal-error
/// on `Err`. The thin wrapper at every leaf's `pub async fn` is:
///
/// ```ignore
/// pub async fn foo(...) -> bool {
///     emit_result(async { /* old ?-chained body */ }.await)
/// }
/// ```
pub fn emit_result<E: std::fmt::Display>(result: Result<Output, E>) -> bool {
    match result {
        Ok(output) => { emit_output(output); true }
        Err(e) => { emit_fatal(e); false }
    }
}
