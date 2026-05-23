//! Console / exception entries captured by the overlay.
//!
//! The overlay monkey-patches `console.log/warn/error/info/debug`
//! on init and installs `window.onerror` + `unhandledrejection`
//! listeners. Each call appends to an in-page ring buffer; the
//! buffer is drained back to the host on a [`crate::request::Request::Console`].
//!
//! Wire format: arrays of [`ConsoleEntry`] objects with the
//! externally-snake-cased [`ConsoleLevel`] tag.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Log,
    Warn,
    Error,
    Info,
    Debug,
    /// Uncaught exception or unhandled promise rejection. Carries a
    /// stack trace in [`ConsoleEntry::stack`] when one is available.
    Exception,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    /// Concatenated message — multiple args to a single `console.log`
    /// call are joined with spaces; non-string args are
    /// `JSON.stringify`'d.
    pub message: String,
    /// `Date.now()` at the time of the call (ms since epoch).
    pub timestamp: f64,
    /// `location.href` of the page that produced the entry. Useful
    /// when the overlay has been remounted across a navigation.
    pub url: String,
    /// Stack trace for `Error` / `Exception` entries; absent
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}
