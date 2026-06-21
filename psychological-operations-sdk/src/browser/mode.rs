//! The session mode the browser process is bound to for its
//! lifetime.
//!
//! Mode is locked at startup by the browser binary's CLI flag
//! (`--x-app` / `--agent-read <tag>` / `--agent-authorize <tag>`
//! / `--agent-browser <tag>`) and held in a process-global
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
    /// Per-agent session in "read" mode — the overlay streams
    /// page HTML to Rust as the user browses the agent's For You
    /// feed, Rust dedups and emits new tweet IDs to stdout, and
    /// the instruction panel surfaces a running "Tweets read: X"
    /// counter. This is the for-you collection mode for `psyops run`.
    AgentRead { name: String },
    /// Per-agent OAuth-authorize session. Rust auto-fires X's
    /// OAuth 2.0 PKCE consent on sign-in and writes the resulting
    /// tokens to `<agent-data-dir>/handles/<twid>/auth.json`. The
    /// same X account can be signed into multiple agents without
    /// any twid-conflict guard.
    AgentAuthorize { name: String },
    /// Per-agent "just browse" session. The webview lands on
    /// `https://x.com/` under the agent's CEF profile; the
    /// operator does whatever they want. No read-scrape, no OAuth
    /// flow. The instruction panel only ever shows `SignInToX` (if
    /// not signed in) or hides entirely. The browser waits for the
    /// operator to close the window.
    AgentBrowser { name: String },
    /// Per-agent reply/quote **delivery** session, scoped to one agent
    /// (the CLI spawns one browser per agent). Shares the agent's
    /// `agent-<tag>` CEF profile; the delivery driver walks the `--items`
    /// payload, driving the overlay to post each reply/quote, and streams
    /// one `Output::Delivered` per success before self-exiting.
    AgentDeliver { name: String },
    /// Discord bot-creation wizard for one agent (`name`). Drives the
    /// Discord developer portal — sign in, create the bot, scrape its
    /// token — and stores the token for `name`. Uses a single shared
    /// `discord` CEF profile (one operator account creates every bot), so
    /// `name` rides along for bot naming + token storage, not the profile
    /// dir.
    DiscordLogin { name: String },
}

/// Reduce a persona name to a SINGLE filesystem path segment for use as
/// its CEF profile directory name. Agent names are always tags and psyop
/// names are flat, so in practice there is nothing to change — but any
/// stray path separator is collapsed to `-` regardless: CEF's Chrome
/// runtime refuses to create a profile whose `cache_path` is not a
/// *direct* child of the cache root ("Cannot create profile at path"),
/// so the per-persona dir must never contain a separator. (An earlier
/// nested `<kind>/<name>` layout hit exactly that.)
fn flat_segment(name: &str) -> String {
    name.replace(['/', '\\'], "-")
}

impl Mode {
    /// The CEF per-context cache subdirectory this mode uses, a DIRECT
    /// child of `browser/cef-root/`. Single source of truth for the
    /// mapping — the browser's webview profile setup, the db crate's
    /// cookie probe, and `reset` all key off this.
    ///
    /// Each persona gets ONE flat directory, `<kind>-<name>`. CEF's
    /// Chrome runtime (the default) only accepts a profile whose
    /// `cache_path` is an *immediate* child of `root_cache_path`; a
    /// nested path makes `ProfileManager` refuse with "Cannot create
    /// profile at path", leaving the persona with no on-disk cookie
    /// store (so its sign-in never persists). Names are flat — agent
    /// tags / psyop names — and [`flat_segment`] collapses any stray
    /// separator as a safety net.
    pub fn cache_subdir(&self) -> String {
        match self {
            Mode::XApp => "x-app".to_string(),
            Mode::AgentRead { name }
            | Mode::AgentAuthorize { name }
            | Mode::AgentBrowser { name }
            | Mode::AgentDeliver { name } => format!("agent-{}", flat_segment(name)),
            // One shared Discord operator profile across all agents — the
            // per-agent thing is the bot + token, not the login session.
            Mode::DiscordLogin { .. } => "discord".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_app_is_flat() {
        assert_eq!(Mode::XApp.cache_subdir(), "x-app");
    }

    #[test]
    fn agent_read_is_a_single_flat_dir() {
        let m = Mode::AgentRead {
            name: "light-yagami".into(),
        };
        assert_eq!(m.cache_subdir(), "agent-light-yagami");
        assert!(!m.cache_subdir().contains('/'));
    }

    #[test]
    fn agent_is_a_single_flat_dir() {
        let m = Mode::AgentAuthorize {
            name: "light-yagami".into(),
        };
        assert_eq!(m.cache_subdir(), "agent-light-yagami");
        assert!(!m.cache_subdir().contains('/'));
    }

    #[test]
    fn same_persona_shares_one_profile_across_submodes() {
        let a = Mode::AgentAuthorize { name: "foo".into() }.cache_subdir();
        let b = Mode::AgentBrowser { name: "foo".into() }.cache_subdir();
        let c = Mode::AgentDeliver { name: "foo".into() }.cache_subdir();
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn stray_separator_never_nests() {
        // Names are flat tags; this is a safety net — a separator must
        // never produce a nested profile dir (Chrome runtime rejects a
        // profile whose cache_path isn't a direct child of the root).
        let m = Mode::AgentAuthorize { name: "a/b".into() };
        assert!(!m.cache_subdir().contains('/'));
        assert!(!m.cache_subdir().contains('\\'));
    }
}
