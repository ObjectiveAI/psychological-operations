//! The session mode the browser is currently in.
//!
//! Mode is the highest-priority piece of state — every [`crate::output::Output`]
//! line carries it as a top-level `"mode"` field so consumers can
//! tell which session produced which event. It's set on the Rust
//! side when a mode-setting [`crate::request::Request`] arrives
//! ([`crate::request::Request::XApp`] today; `Psyop { name }` later)
//! and held in a process-global slot below so [`crate::output::Output::emit`]
//! can read it without taking a host-supplied parameter.
//!
//! The frontend overlay can also query the current mode via the
//! `current_mode` Tauri command — useful for resuming URL reporting
//! after a full-page navigation re-mounts the overlay on a new
//! origin.

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mode {
    /// Master X-App (root) session. Webview lands on
    /// `https://console.x.com/`. Sign-in is observed via cookies on
    /// the content webview; the exact cookie name + domain is in
    /// flux while we're switching off the legacy `console.x.ai` /
    /// `sso`-on-`.x.ai` setup — see the cookies watcher for the
    /// current set of names being probed.
    XApp,
    /// Per-psyop session in "scrape" mode — user just browses
    /// x.com on the persona's cookie jar. Default psyop-mode
    /// behavior; nothing extra runs on top.
    PsyopScrape { name: String },
    /// Per-psyop session in "authorize" mode — Rust drives the
    /// persona through X's OAuth 2.0 PKCE consent screen once
    /// they sign in, captures the access/refresh token pair,
    /// and writes it to
    /// `<psyop-data-dir>/handles/<persona-twid>/auth.json`.
    PsyopAuthorize { name: String },
}

/// Process-global slot for the current mode. Set by [`set`], read by
/// [`get`]. `None` before any mode-setting request has been processed
/// (e.g. during `--help` / clap-error emission).
fn slot() -> &'static Mutex<Option<Mode>> {
    static SLOT: OnceLock<Mutex<Option<Mode>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Update the current mode. Subsequent [`crate::output::Output::emit`]
/// calls will include the new mode in their `"mode"` field.
pub fn set(mode: Option<Mode>) {
    *slot().lock().expect("mode slot poisoned") = mode;
}

/// Read the current mode. Returns `None` before any mode-setting
/// request has been processed.
pub fn get() -> Option<Mode> {
    slot().lock().expect("mode slot poisoned").clone()
}
