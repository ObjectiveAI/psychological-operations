//! The session mode the browser process is bound to for its
//! lifetime.
//!
//! Mode is locked at startup by the browser binary's CLI flag
//! (`--x-app` / `--psyop-read <name>` / `--psyop-authorize <name>`
//! / `--agent-authorize <name>`) and held in a process-global
//! [`OnceLock`] so anything that
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
    /// Per-agent OAuth-authorize session. Mirrors
    /// [`PsyopAuthorize`] operationally — Rust auto-fires X's
    /// OAuth 2.0 PKCE consent on sign-in and writes the
    /// resulting tokens to
    /// `<agent-data-dir>/handles/<twid>/auth.json`. Unlike
    /// psyops, agents don't participate in the twid-conflict
    /// guard: the same X account can be signed into multiple
    /// agents (and into psyops too) simultaneously without
    /// the panel blocking.
    AgentAuthorize { name: String },
    /// Per-psyop "just browse" session. The webview lands on
    /// `https://x.com/` under the psyop's CEF profile; the
    /// operator does whatever they want. No read-scrape, no
    /// OAuth flow, no twid-conflict guard. The instruction
    /// panel only ever shows `SignInToX` (if not signed in) or
    /// hides entirely. The browser waits for the operator to
    /// close the window; the CLI's `psyops browser <name>`
    /// blocks on that exit.
    PsyopBrowser { name: String },
    /// Per-agent "just browse" session. Same shape as
    /// [`PsyopBrowser`], rooted under the agent's CEF profile.
    AgentBrowser { name: String },
}

impl Mode {
    /// The flat CEF per-context cache subdirectory this mode uses under
    /// `browser/cef-root/`. Single source of truth for the mapping —
    /// the browser's webview profile setup and the db crate's cookie
    /// probe both key off this. Must stay flat (no nested slashes): the
    /// CEF Chrome runtime silently falls back to an in-memory profile if
    /// `cache_path` contains path separators.
    pub fn cache_subdir(&self) -> String {
        match self {
            Mode::XApp => "x-app".to_string(),
            Mode::PsyopRead { name }
            | Mode::PsyopAuthorize { name }
            | Mode::PsyopBrowser { name } => format!("psyop-{name}"),
            Mode::AgentAuthorize { name } | Mode::AgentBrowser { name } => {
                format!("agent-{name}")
            }
        }
    }
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
