//! The session mode the browser process is bound to for its
//! lifetime.
//!
//! Mode is locked at startup by the browser binary's CLI flag
//! (`--x-app` / `--psyop-read <name>` / `--psyop-authorize <name>`)
//! and held in a process-global [`OnceLock`] so anything that
//! needs it can read it without a host-supplied parameter.
//! There is no runtime way to change mode — to switch, kill the
//! process and relaunch with a different flag.

use std::sync::OnceLock;

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
    /// Per-psyop session in "read" mode — the overlay streams
    /// page HTML to Rust as the user browses, Rust dedups and
    /// emits new tweet IDs to stdout, and the instruction
    /// panel surfaces a running "Tweets read: X" counter.
    PsyopRead { name: String },
    /// Per-psyop session in "authorize" mode — Rust drives the
    /// persona through X's OAuth 2.0 PKCE consent screen once
    /// they sign in, captures the access/refresh token pair,
    /// and writes it to
    /// `<psyop-data-dir>/handles/<persona-twid>/auth.json`.
    PsyopAuthorize { name: String },
}

/// Process-global once-only slot for the session mode.
fn slot() -> &'static OnceLock<Mode> {
    static SLOT: OnceLock<Mode> = OnceLock::new();
    &SLOT
}

/// Lock the session mode for the lifetime of the process.
/// First call wins; subsequent calls are silently ignored.
pub fn set(mode: Mode) {
    let _ = slot().set(mode);
}

/// Read the current mode. `None` before [`set`] has run —
/// callers handle that case (e.g. `--help` / clap-error
/// emission happens before mode is locked in).
pub fn get() -> Option<Mode> {
    slot().get().cloned()
}
