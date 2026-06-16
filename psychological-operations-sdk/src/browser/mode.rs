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

/// Directory interspersed between agent-instance-hierarchy levels in the
/// CEF cache layout (and the credential data dir). For AIH
/// `foo/bar/buzz` the path becomes `foo/agents/bar/agents/buzz`, so:
///   * each persona's own Chromium profile artifacts (`Network/`,
///     `Cache/`, …) live directly in its node dir, while its descendant
///     personas live under the node's `agents/` child — disjoint
///     namespaces, so a child AIH segment can never collide with a
///     parent's profile artifact (e.g. an agent `foo/Cache` lands at
///     `…/foo/agents/Cache`, not `…/foo/Cache`); and
///   * `--dangerously-reset` can wipe a persona's own profile while
///     sparing this folder, leaving every descendant persona intact
///     (see `reset::wipe_persona`).
/// `agents` is never a Chromium profile artifact name, so sparing it on
/// reset never spares the persona's own data.
pub const SUBAGENT_DIR: &str = "agents";

/// Intersperse [`SUBAGENT_DIR`] between the '/'-separated levels of an
/// AIH: `a/b/c` → `a/agents/b/agents/c`. A slashless name is returned
/// unchanged.
fn intersperse_subagents(aih: &str) -> String {
    aih.split('/')
        .collect::<Vec<_>>()
        .join(&format!("/{SUBAGENT_DIR}/"))
}

impl Mode {
    /// The CEF per-context cache subdirectory this mode uses under
    /// `browser/cef-root/`. Single source of truth for the mapping —
    /// the browser's webview profile setup, the db crate's cookie probe,
    /// and `reset` all key off this.
    ///
    /// Agent personas use the full agent-instance-hierarchy with
    /// [`SUBAGENT_DIR`] interspersed between levels (see its docs), so a
    /// hierarchy nests into real directories whose layout is
    /// reset-safe and collision-free. CEF (alloy runtime) accepts any
    /// descendant of `root_cache_path` as a persistent on-disk profile —
    /// depth doesn't matter — and the consumers normalize `/`→`\` on
    /// Windows before handing the path to CEF (`cef::path_to_cef_string`).
    pub fn cache_subdir(&self) -> String {
        match self {
            Mode::XApp => "x-app".to_string(),
            // Psyop names are flat (a single `psyops publish --name`), so
            // there's nothing to intersperse.
            Mode::PsyopRead { name }
            | Mode::PsyopAuthorize { name }
            | Mode::PsyopBrowser { name } => format!("psyop/{name}"),
            Mode::AgentAuthorize { name } | Mode::AgentBrowser { name } => {
                format!("agent/{}", intersperse_subagents(name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_app_is_flat() {
        assert_eq!(Mode::XApp.cache_subdir(), "x-app");
    }

    #[test]
    fn psyop_is_flat() {
        let m = Mode::PsyopAuthorize {
            name: "my-psyop".into(),
        };
        assert_eq!(m.cache_subdir(), "psyop/my-psyop");
    }

    #[test]
    fn single_segment_agent_has_no_separator() {
        let m = Mode::AgentAuthorize { name: "foo".into() };
        assert_eq!(m.cache_subdir(), "agent/foo");
    }

    #[test]
    fn nested_aih_intersperses_agents() {
        let m = Mode::AgentAuthorize {
            name: "foo/bar/buzz".into(),
        };
        assert_eq!(m.cache_subdir(), "agent/foo/agents/bar/agents/buzz");
        // Same persona across its browse/authorize sub-modes shares one
        // profile dir.
        let b = Mode::AgentBrowser {
            name: "foo/bar/buzz".into(),
        };
        assert_eq!(b.cache_subdir(), m.cache_subdir());
    }

    #[test]
    fn child_segment_named_like_a_chromium_artifact_lands_under_agents() {
        // `foo`'s own Cache is `agent/foo/Cache`; the child `foo/Cache`
        // lands under the `agents/` folder — no collision.
        let parent = Mode::AgentAuthorize { name: "foo".into() }.cache_subdir();
        let child = Mode::AgentAuthorize {
            name: "foo/Cache".into(),
        }
        .cache_subdir();
        assert_eq!(parent, "agent/foo");
        assert_eq!(child, "agent/foo/agents/Cache");
        assert!(child.starts_with(&format!("{parent}/{SUBAGENT_DIR}/")));
    }
}
